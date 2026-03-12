//! Process Management for Persistent Assistants
//!
//! This module handles spawning assistant processes as separate child processes,
//! managing IPC communication, and monitoring process health.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{interval, timeout};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{AssistantConfig, AssistantStatus, PersistentAssistant};

/// IPC message types between parent and child assistants
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcMessage {
    /// Request to process a message
    ProcessRequest {
        request_id: String,
        message: String,
        context: HashMap<String, serde_json::Value>,
    },
    /// Response to a process request
    ProcessResponse {
        request_id: String,
        response: String,
        success: bool,
        error: Option<String>,
    },
    /// Health check ping
    Ping { request_id: String },
    /// Health check pong
    Pong { request_id: String },
    /// Status update from child
    StatusUpdate {
        status: AssistantStatus,
        info: Option<String>,
    },
    /// Shutdown request
    Shutdown { reason: Option<String> },
    /// Log message from child
    Log {
        level: String,
        message: String,
    },
}

/// Handle to a spawned assistant process
#[derive(Debug)]
pub struct AssistantProcess {
    /// Assistant ID
    pub assistant_id: String,
    /// The child process
    child: Arc<Mutex<Child>>,
    /// Channel for sending IPC messages to the child
    tx: mpsc::UnboundedSender<IpcMessage>,
    /// Response handlers for pending requests
    pending_responses: Arc<RwLock<HashMap<String, mpsc::Sender<IpcMessage>>>>,
    /// Process handle for the health monitor
    _monitor_handle: tokio::task::JoinHandle<()>,
}

impl AssistantProcess {
    /// Spawn a new assistant process
    pub async fn spawn(
        assistant: &PersistentAssistant,
        config: &AssistantConfig,
    ) -> crate::Result<Self> {
        let assistant_id = assistant.id.clone();
        let data_dir = assistant.data_dir.clone();

        // Get the current executable path (manta binary)
        let current_exe = std::env::current_exe()
            .map_err(|e| crate::error::MantaError::Internal(format!("Failed to get current exe: {}", e)))?;

        // Prepare environment variables for the child
        let mut envs = config.environment.clone();
        envs.insert("MANTA_ASSISTANT_ID".to_string(), assistant_id.clone());
        envs.insert("MANTA_ASSISTANT_NAME".to_string(), assistant.name.clone());
        envs.insert("MANTA_ASSISTANT_TYPE".to_string(), format!("{}", assistant.assistant_type));
        envs.insert("MANTA_ASSISTANT_DATA_DIR".to_string(), data_dir.to_string_lossy().to_string());
        envs.insert("MANTA_PARENT_ASSISTANT_ID".to_string(), assistant.parent_id.clone().unwrap_or_default());
        envs.insert("MANTA_MODE".to_string(), "assistant_process".to_string());

        // Set resource limits in environment
        envs.insert("MANTA_MAX_ITERATIONS".to_string(), config.resource_limits.max_iterations.to_string());
        envs.insert("MANTA_MAX_MEMORY_MB".to_string(), config.resource_limits.max_memory_mb.to_string());
        envs.insert("MANTA_MAX_REQUESTS_PER_MINUTE".to_string(), config.resource_limits.max_requests_per_minute.to_string());

        debug!("Spawning assistant process: {} at {:?}", assistant_id, data_dir);

        // Spawn the child process
        let mut child = Command::new(&current_exe)
            .arg("assistant-run")
            .arg("--config")
            .arg(data_dir.join("config.yaml"))
            .current_dir(&data_dir)
            .envs(&envs)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| crate::error::MantaError::Internal(format!(
                "Failed to spawn assistant process: {}", e
            )))?;

        // Get stdin/stdout handles
        let stdin = child.stdin.take()
            .ok_or_else(|| crate::error::MantaError::Internal("Failed to get stdin".to_string()))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| crate::error::MantaError::Internal("Failed to get stdout".to_string()))?;

        // Create channels for IPC
        let (tx, mut rx) = mpsc::unbounded_channel::<IpcMessage>();
        let pending_responses: Arc<RwLock<HashMap<String, mpsc::Sender<IpcMessage>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Spawn stdin writer task
        let assistant_id_clone = assistant_id.clone();
        let mut stdin = stdin;
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let json = match serde_json::to_string(&msg) {
                    Ok(j) => j,
                    Err(e) => {
                        error!("Failed to serialize IPC message: {}", e);
                        continue;
                    }
                };

                if let Err(e) = stdin.write_all(json.as_bytes()).await {
                    error!("Failed to write to assistant {} stdin: {}", assistant_id_clone, e);
                    break;
                }
                if let Err(e) = stdin.write_all(b"\n").await {
                    error!("Failed to write newline to assistant {} stdin: {}", assistant_id_clone, e);
                    break;
                }
                if let Err(e) = stdin.flush().await {
                    error!("Failed to flush assistant {} stdin: {}", assistant_id_clone, e);
                    break;
                }
            }
            debug!("stdin writer for assistant {} closed", assistant_id_clone);
        });

        // Spawn stdout reader task
        let pending_clone = pending_responses.clone();
        let assistant_id_reader = assistant_id.clone();
        let mut reader = BufReader::new(stdout).lines();

        tokio::spawn(async move {
            while let Ok(Some(line)) = reader.next_line().await {
                debug!("Received from assistant {}: {}", assistant_id_reader, line);

                match serde_json::from_str::<IpcMessage>(&line) {
                    Ok(msg) => {
                        // Handle responses
                        if let Some(request_id) = msg.request_id() {
                            let pending = pending_clone.read().await;
                            if let Some(response_tx) = pending.get(&request_id) {
                                let _ = response_tx.send(msg).await;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to parse IPC message from {}: {}", assistant_id_reader, e);
                    }
                }
            }
            debug!("stdout reader for assistant {} closed", assistant_id_reader);
        });

        // Spawn stderr logger
        let assistant_id_stderr = assistant_id.clone();
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    warn!("Assistant {} stderr: {}", assistant_id_stderr, line);
                }
            });
        }

        // Clone tx for health monitor
        let tx_monitor = tx.clone();

        // Spawn health monitor
        let child_arc = Arc::new(Mutex::new(child));
        let child_monitor = child_arc.clone();
        let pending_monitor = pending_responses.clone();
        let assistant_id_monitor = assistant_id.clone();
        let monitor_handle = tokio::spawn(async move {
            let mut check_interval = interval(Duration::from_secs(30));

            loop {
                check_interval.tick().await;

                // Send ping
                let ping_id = Uuid::new_v4().to_string();
                let (pong_tx, mut pong_rx) = mpsc::channel(1);

                {
                    let mut pending = pending_monitor.write().await;
                    pending.insert(ping_id.clone(), pong_tx);
                }

                let ping = IpcMessage::Ping { request_id: ping_id };
                let _ = tx_monitor.send(ping);

                // Wait for pong with timeout
                match timeout(Duration::from_secs(10), pong_rx.recv()).await {
                    Ok(Some(_)) => {
                        // Healthy
                        debug!("Assistant {} health check passed", assistant_id_monitor);
                    }
                    _ => {
                        warn!("Assistant {} health check failed", assistant_id_monitor);
                        // Check if process is still running
                        let mut child = child_monitor.lock().await;
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                warn!("Assistant {} process exited with: {:?}", assistant_id_monitor, status);
                                break;
                            }
                            Ok(None) => {
                                // Process still running but not responding
                                warn!("Assistant {} not responding, considering restart", assistant_id_monitor);
                            }
                            Err(e) => {
                                error!("Failed to check assistant {} status: {}", assistant_id_monitor, e);
                                break;
                            }
                        }
                    }
                }
            }
        });

        info!("Assistant process {} spawned successfully", assistant_id);

        Ok(Self {
            assistant_id,
            child: child_arc,
            tx,
            pending_responses,
            _monitor_handle: monitor_handle,
        })
    }

    /// Send a message to the assistant and wait for response
    pub async fn send_message(
        &self,
        message: &str,
        context: HashMap<String, serde_json::Value>,
    ) -> crate::Result<String> {
        let request_id = Uuid::new_v4().to_string();
        let (response_tx, mut response_rx) = mpsc::channel(1);

        // Register response handler
        {
            let mut pending = self.pending_responses.write().await;
            pending.insert(request_id.clone(), response_tx);
        }

        // Send request
        let request = IpcMessage::ProcessRequest {
            request_id: request_id.clone(),
            message: message.to_string(),
            context,
        };

        self.tx.send(request).map_err(|_| {
            crate::error::MantaError::Internal("Failed to send message to assistant".to_string())
        })?;

        // Wait for response with timeout
        match timeout(Duration::from_secs(60), response_rx.recv()).await {
            Ok(Some(IpcMessage::ProcessResponse { response, success, error, .. })) => {
                if success {
                    Ok(response)
                } else {
                    Err(crate::error::MantaError::ExternalService {
                        source: format!("assistant-{}: {}", self.assistant_id, error.unwrap_or_else(|| "Unknown error".to_string())),
                        cause: None,
                    })
                }
            }
            Ok(Some(_)) => {
                Err(crate::error::MantaError::Internal("Unexpected response type".to_string()))
            }
            Ok(None) => {
                Err(crate::error::MantaError::Internal("Response channel closed".to_string()))
            }
            Err(_) => {
                Err(crate::error::MantaError::ExternalService {
                    source: format!("assistant-{}: Request timeout", self.assistant_id),
                    cause: None,
                })
            }
        }
    }

    /// Gracefully shutdown the assistant
    pub async fn shutdown(&self, reason: Option<String>) -> crate::Result<()> {
        // Send shutdown message
        let shutdown = IpcMessage::Shutdown { reason };
        let _ = self.tx.send(shutdown);

        // Wait a bit for graceful shutdown
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Force kill if still running
        let mut child = self.child.lock().await;
        match child.try_wait() {
            Ok(None) => {
                // Still running, kill it
                warn!("Assistant {} didn't shutdown gracefully, killing", self.assistant_id);
                let _ = child.kill().await;
            }
            _ => {}
        }

        Ok(())
    }

    /// Check if the process is still running
    pub async fn is_running(&self) -> bool {
        let mut child = self.child.lock().await;
        matches!(child.try_wait(), Ok(None))
    }
}

impl IpcMessage {
    /// Get the request ID if this message has one
    fn request_id(&self) -> Option<String> {
        match self {
            IpcMessage::ProcessRequest { request_id, .. } => Some(request_id.clone()),
            IpcMessage::ProcessResponse { request_id, .. } => Some(request_id.clone()),
            IpcMessage::Ping { request_id } => Some(request_id.clone()),
            IpcMessage::Pong { request_id } => Some(request_id.clone()),
            _ => None,
        }
    }
}

/// Process manager for all assistant processes
#[derive(Debug, Default)]
pub struct ProcessManager {
    /// Running processes
    processes: Arc<RwLock<HashMap<String, AssistantProcess>>>,
}

impl ProcessManager {
    /// Create a new process manager
    pub fn new() -> Self {
        Self {
            processes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start an assistant process
    pub async fn start(
        &self,
        assistant: &PersistentAssistant,
        config: &AssistantConfig,
    ) -> crate::Result<()> {
        let process = AssistantProcess::spawn(assistant, config).await?;

        let mut processes = self.processes.write().await;
        processes.insert(assistant.id.clone(), process);

        Ok(())
    }

    /// Send message to an assistant
    pub async fn send_message(
        &self,
        assistant_id: &str,
        message: &str,
        context: HashMap<String, serde_json::Value>,
    ) -> crate::Result<String> {
        let processes = self.processes.read().await;
        let process = processes.get(assistant_id).ok_or_else(|| {
            crate::error::MantaError::NotFound {
                resource: format!("Running assistant {}", assistant_id),
            }
        })?;

        process.send_message(message, context).await
    }

    /// Stop an assistant process
    pub async fn stop(&self, assistant_id: &str, reason: Option<String>) -> crate::Result<()> {
        let mut processes = self.processes.write().await;
        if let Some(process) = processes.remove(assistant_id) {
            process.shutdown(reason).await?;
        }
        Ok(())
    }

    /// Check if an assistant is running
    pub async fn is_running(&self, assistant_id: &str) -> bool {
        let processes = self.processes.read().await;
        if let Some(process) = processes.get(assistant_id) {
            process.is_running().await
        } else {
            false
        }
    }

    /// Get list of running assistant IDs
    pub async fn running_ids(&self) -> Vec<String> {
        let processes = self.processes.read().await;
        processes.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_message_serialization() {
        let msg = IpcMessage::ProcessRequest {
            request_id: "test-123".to_string(),
            message: "Hello".to_string(),
            context: HashMap::new(),
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("process_request"));
        assert!(json.contains("test-123"));

        let decoded: IpcMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            IpcMessage::ProcessRequest { request_id, message, .. } => {
                assert_eq!(request_id, "test-123");
                assert_eq!(message, "Hello");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_ipc_message_request_id() {
        let ping = IpcMessage::Ping {
            request_id: "ping-1".to_string(),
        };
        assert_eq!(ping.request_id(), Some("ping-1".to_string()));

        let log = IpcMessage::Log {
            level: "info".to_string(),
            message: "test".to_string(),
        };
        assert_eq!(log.request_id(), None);
    }
}
