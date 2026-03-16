//! Agent Control Plane (ACP) - Subagent Spawning System
//!
//! Inspired by OpenClaw's ACP, this provides:
//! - Subagent spawning with thread binding
//! - Runtime modes: "run" (one-shot) vs "session" (persistent)
//! - Session actor queue for serialized execution
//! - Parent-child agent communication

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::agent::{Agent, AgentConfig};
use crate::channels::IncomingMessage;
use crate::tools::ToolRegistry;

// AgentHandle is defined in gateway module
pub use crate::gateway::AgentHandle;

/// ACP Session ID - unique identifier for an ACP session
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AcpSessionId(pub String);

impl AcpSessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for AcpSessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AcpSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Subagent spawn mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnMode {
    /// One-shot execution (run and terminate)
    Run,
    /// Persistent session (long-running)
    Session,
}

impl Default for SpawnMode {
    fn default() -> Self {
        SpawnMode::Run
    }
}

/// Thread binding mode for subagents
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadBinding {
    /// New isolated thread
    New,
    /// Bind to parent's thread
    Parent,
    /// Bind to specific thread ID
    Thread(String),
    /// Automatic based on context
    Auto,
}

impl Default for ThreadBinding {
    fn default() -> Self {
        ThreadBinding::Auto
    }
}

/// Subagent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentConfig {
    /// Agent type/personality to use
    pub agent_type: String,
    /// Spawn mode
    pub mode: SpawnMode,
    /// Thread binding
    pub thread_binding: ThreadBinding,
    /// System prompt override
    pub system_prompt: Option<String>,
    /// Maximum tokens
    pub max_tokens: Option<usize>,
    /// Temperature
    pub temperature: Option<f32>,
    /// Tools to enable
    pub tools: Vec<String>,
    /// Initial context/data
    pub context: Option<serde_json::Value>,
    /// Timeout in seconds (for Run mode)
    pub timeout_seconds: Option<u64>,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            agent_type: "default".to_string(),
            mode: SpawnMode::Run,
            thread_binding: ThreadBinding::Auto,
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            tools: vec![],
            context: None,
            timeout_seconds: Some(300),
        }
    }
}

/// Subagent handle - reference to a spawned subagent
#[derive(Debug, Clone)]
pub struct SubagentHandle {
    /// Subagent ID
    pub id: String,
    /// Parent agent ID
    pub parent_id: String,
    /// ACP Session ID
    pub session_id: AcpSessionId,
    /// Spawn mode
    pub mode: SpawnMode,
    /// Thread ID this agent is bound to
    pub thread_id: String,
    /// Command channel to subagent
    pub command_tx: mpsc::Sender<SubagentCommand>,
    /// Current status
    pub status: SubagentStatus,
}

/// Subagent status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    /// Starting up
    Starting,
    /// Ready for work
    Ready,
    /// Busy processing
    Busy,
    /// Shutting down
    ShuttingDown,
    /// Terminated
    Terminated,
}

/// Commands that can be sent to a subagent
#[derive(Debug)]
pub enum SubagentCommand {
    /// Process a message
    ProcessMessage {
        message: IncomingMessage,
        response_tx: oneshot::Sender<crate::Result<String>>,
    },
    /// Update configuration
    UpdateConfig(AgentConfig),
    /// Cancel current operation
    Cancel,
    /// Shutdown the subagent
    Shutdown,
}

impl Clone for SubagentCommand {
    fn clone(&self) -> Self {
        match self {
            Self::UpdateConfig(config) => Self::UpdateConfig(config.clone()),
            Self::Cancel => Self::Cancel,
            Self::Shutdown => Self::Shutdown,
            // ProcessMessage can't be cloned due to oneshot, convert to Cancel
            Self::ProcessMessage { .. } => Self::Cancel,
        }
    }
}

/// Response from a subagent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResponse {
    pub subagent_id: String,
    pub result: Result<String, String>,
    pub metadata: Option<serde_json::Value>,
}

/// Thread context for serialized execution
#[derive(Debug)]
pub struct ThreadContext {
    /// Thread ID
    pub id: String,
    /// Active subagent on this thread (if any)
    pub active_subagent: Option<String>,
    /// Message queue for this thread
    pub queue: Vec<ThreadMessage>,
    /// Created timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Message in a thread queue
#[derive(Debug)]
pub struct ThreadMessage {
    pub id: String,
    pub subagent_id: String,
    pub message: IncomingMessage,
    pub response_tx: Option<oneshot::Sender<crate::Result<String>>>,
    pub queued_at: chrono::DateTime<chrono::Utc>,
}

/// ACP Control Plane - manages subagents and threads
pub struct AcpControlPlane {
    /// Subagents by ID
    subagents: Arc<RwLock<HashMap<String, SubagentHandle>>>,
    /// Threads by ID
    threads: Arc<RwLock<HashMap<String, ThreadContext>>>,
    /// ACP sessions
    sessions: Arc<RwLock<HashMap<AcpSessionId, AcpSession>>>,
    /// Default agent builder
    default_agent_builder: Option<Arc<dyn Fn() -> crate::Result<Agent> + Send + Sync>>,
}

/// ACP Session - groups related subagents
#[derive(Debug)]
pub struct AcpSession {
    pub id: AcpSessionId,
    pub parent_agent_id: String,
    pub subagents: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl AcpControlPlane {
    /// Create a new ACP control plane
    pub fn new() -> Self {
        Self {
            subagents: Arc::new(RwLock::new(HashMap::new())),
            threads: Arc::new(RwLock::new(HashMap::new())),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            default_agent_builder: None,
        }
    }

    /// Set the default agent builder
    pub fn with_agent_builder<F>(mut self, builder: F) -> Self
    where
        F: Fn() -> crate::Result<Agent> + Send + Sync + 'static,
    {
        self.default_agent_builder = Some(Arc::new(builder));
        self
    }

    /// Create a new ACP session
    pub async fn create_session(
        &self,
        parent_agent_id: String,
    ) -> AcpSessionId {
        let session_id = AcpSessionId::new();
        let session = AcpSession {
            id: session_id.clone(),
            parent_agent_id,
            subagents: vec![],
            created_at: chrono::Utc::now(),
        };

        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.clone(), session);

        info!("Created ACP session {}", session_id);
        session_id
    }

    /// Spawn a subagent
    pub async fn spawn_subagent(
        &self,
        session_id: AcpSessionId,
        parent_id: String,
        config: SubagentConfig,
    ) -> crate::Result<SubagentHandle> {
        let subagent_id = format!("subagent-{}", Uuid::new_v4());
        let thread_id = self.resolve_thread_id(&config.thread_binding, &parent_id).await;

        info!(
            "Spawning subagent {} (mode: {:?}, thread: {})",
            subagent_id, config.mode, thread_id
        );

        // Create command channel
        let (command_tx, mut command_rx) = mpsc::channel::<SubagentCommand>(100);

        // Build agent config
        let agent_config = AgentConfig {
            system_prompt: config.system_prompt.unwrap_or_default(),
            max_tokens: config.max_tokens.map(|m| m as u32).unwrap_or(2048),
            max_context_tokens: 4096,
            max_concurrent_tools: 5,
            temperature: config.temperature.unwrap_or(0.7),
            skills_prompt: None,
        };

        // Create the agent
        let agent = if let Some(ref builder) = self.default_agent_builder {
            builder()?
        } else {
            return Err(crate::error::MantaError::Internal(
                "No agent builder configured".to_string()
            ));
        };

        // Spawn subagent task
        let subagent_id_clone = subagent_id.clone();
        let mode = config.mode;
        let timeout = config.timeout_seconds;

        tokio::spawn(async move {
            info!("Subagent {} task started", subagent_id_clone);

            while let Some(cmd) = command_rx.recv().await {
                match cmd {
                    SubagentCommand::ProcessMessage { message, response_tx } => {
                        debug!("Subagent {} processing message", subagent_id_clone);

                        let result = tokio::time::timeout(
                            std::time::Duration::from_secs(timeout.unwrap_or(300)),
                            async {
                                // Process the message through the agent
                                agent.process_message(message).await
                            }
                        ).await;

                        let response = match result {
                            Ok(Ok(response)) => Ok(response.content),
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(_) => Err("Timeout".to_string()),
                        };

                        let _ = response_tx.send(response.map_err(|e| {
                            crate::error::MantaError::Internal(e)
                        }));

                        // For Run mode, terminate after first message
                        if mode == SpawnMode::Run {
                            info!("Subagent {} (Run mode) completing", subagent_id_clone);
                            break;
                        }
                    }
                    SubagentCommand::UpdateConfig(_new_config) => {
                        debug!("Subagent {} config updated", subagent_id_clone);
                    }
                    SubagentCommand::Cancel => {
                        debug!("Subagent {} cancelled", subagent_id_clone);
                    }
                    SubagentCommand::Shutdown => {
                        info!("Subagent {} shutting down", subagent_id_clone);
                        break;
                    }
                }
            }

            info!("Subagent {} task ended", subagent_id_clone);
        });

        let handle = SubagentHandle {
            id: subagent_id.clone(),
            parent_id: parent_id.clone(),
            session_id: session_id.clone(),
            mode: config.mode,
            thread_id: thread_id.clone(),
            command_tx,
            status: SubagentStatus::Ready,
        };

        // Register subagent
        let mut subagents = self.subagents.write().await;
        subagents.insert(subagent_id.clone(), handle.clone());

        // Register with session
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&session_id) {
            session.subagents.push(subagent_id.clone());
        }

        // Ensure thread exists
        let mut threads = self.threads.write().await;
        if !threads.contains_key(&thread_id) {
            threads.insert(
                thread_id.clone(),
                ThreadContext {
                    id: thread_id.clone(),
                    active_subagent: None,
                    queue: vec![],
                    created_at: chrono::Utc::now(),
                },
            );
        }

        info!("Subagent {} spawned successfully", subagent_id);
        Ok(handle)
    }

    /// Resolve thread ID based on binding mode
    async fn resolve_thread_id(
        &self,
        binding: &ThreadBinding,
        parent_id: &str,
    ) -> String {
        match binding {
            ThreadBinding::New => format!("thread-{}", Uuid::new_v4()),
            ThreadBinding::Parent => format!("thread-{}", parent_id),
            ThreadBinding::Thread(id) => id.clone(),
            ThreadBinding::Auto => {
                // Check if parent has a thread
                let subagents = self.subagents.read().await;
                if let Some(parent) = subagents.get(parent_id) {
                    parent.thread_id.clone()
                } else {
                    format!("thread-{}", parent_id)
                }
            }
        }
    }

    /// Send a message to a subagent
    pub async fn send_message(
        &self,
        subagent_id: &str,
        message: IncomingMessage,
    ) -> crate::Result<String> {
        let subagents = self.subagents.read().await;
        let subagent = subagents.get(subagent_id).ok_or_else(|| {
            crate::error::MantaError::NotFound {
                resource: format!("Subagent '{}'", subagent_id),
            }
        })?;

        let (response_tx, response_rx) = oneshot::channel();

        subagent
            .command_tx
            .send(SubagentCommand::ProcessMessage {
                message,
                response_tx,
            })
            .await
            .map_err(|_| {
                crate::error::MantaError::Internal(
                    "Subagent command channel closed".to_string()
                )
            })?;

        let result = response_rx.await.map_err(|_| {
            crate::error::MantaError::Internal(
                "Subagent response channel closed".to_string()
            )
        })??;

        Ok(result)
    }

    /// Shutdown a subagent
    pub async fn shutdown_subagent(
        &self,
        subagent_id: &str,
    ) -> crate::Result<bool> {
        let subagents = self.subagents.read().await;

        if let Some(subagent) = subagents.get(subagent_id) {
            let _ = subagent.command_tx.send(SubagentCommand::Shutdown).await;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Terminate all subagents in a session
    pub async fn terminate_session(
        &self,
        session_id: &AcpSessionId,
    ) -> crate::Result<usize> {
        let sessions = self.sessions.read().await;
        let session = sessions.get(session_id).ok_or_else(|| {
            crate::error::MantaError::NotFound {
                resource: format!("Session '{}'", session_id),
            }
        })?;

        let subagent_ids: Vec<String> = session.subagents.clone();
        drop(sessions);

        let mut count = 0;
        for subagent_id in subagent_ids {
            if self.shutdown_subagent(&subagent_id).await? {
                count += 1;
            }
        }

        // Remove session
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);

        info!("Terminated {} subagents in session {}", count, session_id);
        Ok(count)
    }

    /// Get subagent status
    pub async fn get_subagent_status(
        &self,
        subagent_id: &str,
    ) -> Option<SubagentStatus> {
        let subagents = self.subagents.read().await;
        subagents.get(subagent_id).map(|s| s.status)
    }

    /// List all subagents
    pub async fn list_subagents(&self) -> Vec<SubagentHandle> {
        let subagents = self.subagents.read().await;
        subagents.values().cloned().collect()
    }

    /// List subagents in a session
    pub async fn list_session_subagents(
        &self,
        session_id: &AcpSessionId,
    ) -> Vec<SubagentHandle> {
        let sessions = self.sessions.read().await;
        let subagents = self.subagents.read().await;

        if let Some(session) = sessions.get(session_id) {
            session
                .subagents
                .iter()
                .filter_map(|id| subagents.get(id).cloned())
                .collect()
        } else {
            vec![]
        }
    }

    /// Get session info
    pub async fn get_session_info(
        &self,
        session_id: &AcpSessionId,
    ) -> Option<AcpSessionInfo> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| AcpSessionInfo {
            id: s.id.clone(),
            parent_agent_id: s.parent_agent_id.clone(),
            subagent_count: s.subagents.len(),
            created_at: s.created_at,
        })
    }
}

impl Default for AcpControlPlane {
    fn default() -> Self {
        Self::new()
    }
}

/// Session info for display
#[derive(Debug, Clone)]
pub struct AcpSessionInfo {
    pub id: AcpSessionId,
    pub parent_agent_id: String,
    pub subagent_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Extension trait for Agent to support ACP
#[async_trait]
pub trait AcpAgentExt {
    /// Spawn a subagent from this agent
    async fn spawn_subagent(
        &self,
        acp: &AcpControlPlane,
        config: SubagentConfig,
    ) -> crate::Result<SubagentHandle>;
}

#[async_trait]
impl AcpAgentExt for AgentHandle {
    async fn spawn_subagent(
        &self,
        acp: &AcpControlPlane,
        config: SubagentConfig,
    ) -> crate::Result<SubagentHandle> {
        let session_id = AcpSessionId::new();
        acp.spawn_subagent(session_id, self.id.clone(), config).await
    }
}
