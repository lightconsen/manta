//! ACP (Agent Control Plane) Tool - Subagent Spawning
//!
//! This tool allows agents to spawn subagents for parallel task execution.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tracing::{info, warn};

use crate::acp::{AcpControlPlane, AcpSessionId, SpawnMode, SubagentConfig, ThreadBinding};
use crate::channels::IncomingMessage;

use super::{Tool, ToolContext, ToolExecutionResult};

/// Tool for spawning subagents via ACP
pub struct AcpSpawnTool {
    acp: Arc<AcpControlPlane>,
}

impl AcpSpawnTool {
    /// Create a new ACP spawn tool
    pub fn new(acp: Arc<AcpControlPlane>) -> Self {
        Self { acp }
    }
}

/// Arguments for the spawn_subagent tool
#[derive(Debug, Deserialize)]
struct SpawnSubagentArgs {
    /// The task/prompt for the subagent
    pub task: String,
    /// Spawn mode: "run" (one-shot) or "session" (persistent)
    #[serde(default = "default_spawn_mode")]
    pub mode: String,
    /// Thread binding: "new", "parent", "auto", or specific thread ID
    #[serde(default = "default_thread_binding")]
    pub thread_binding: String,
    /// Agent type/personality (e.g., "coder", "researcher", "default")
    #[serde(default)]
    pub agent_type: String,
    /// Maximum execution time in seconds
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

fn default_spawn_mode() -> String {
    "run".to_string()
}

fn default_thread_binding() -> String {
    "auto".to_string()
}

#[async_trait]
impl Tool for AcpSpawnTool {
    fn name(&self) -> &str {
        "spawn_subagent"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to handle a specific task. The subagent can operate in 'run' mode (one-shot execution) or 'session' mode (persistent conversation). Use this for parallel task execution or delegating work to specialized agents."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task or prompt to give to the subagent"
                },
                "mode": {
                    "type": "string",
                    "enum": ["run", "session"],
                    "description": "Spawn mode: 'run' for one-shot execution, 'session' for persistent agent",
                    "default": "run"
                },
                "thread_binding": {
                    "type": "string",
                    "description": "Thread binding: 'new' for isolated thread, 'parent' to bind to parent, 'auto' for automatic",
                    "default": "auto"
                },
                "agent_type": {
                    "type": "string",
                    "description": "Type of agent to spawn (e.g., 'coder', 'researcher', 'default')",
                    "default": "default"
                },
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Maximum execution time in seconds (only for run mode)",
                    "minimum": 1,
                    "maximum": 3600
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();

        let args: SpawnSubagentArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Invalid arguments: {}", e)),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        };

        // Parse spawn mode
        let mode = match args.mode.as_str() {
            "session" => SpawnMode::Session,
            _ => SpawnMode::Run,
        };

        // Parse thread binding
        let thread_binding = match args.thread_binding.as_str() {
            "new" => ThreadBinding::New,
            "parent" => ThreadBinding::Parent,
            "auto" => ThreadBinding::Auto,
            id => ThreadBinding::Thread(id.to_string()),
        };

        // Create ACP session
        let session_id = AcpSessionId::new();
        let parent_id = format!("agent-{}", context.conversation_id);

        // Build subagent config
        let config = SubagentConfig {
            agent_type: if args.agent_type.is_empty() {
                "default".to_string()
            } else {
                args.agent_type
            },
            mode,
            thread_binding,
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            tools: vec![],
            context: None,
            timeout_seconds: args.timeout_seconds.or(Some(300)),
        };

        info!("Spawning subagent for task: {} (mode: {:?})", args.task, config.mode);

        // Spawn the subagent
        match self
            .acp
            .spawn_subagent(session_id.clone(), parent_id.clone(), config)
            .await
        {
            Ok(handle) => {
                let subagent_id = handle.id.clone();

                // Create message for the subagent
                let message = IncomingMessage::new(
                    context.user_id.clone(),
                    context.conversation_id.clone(),
                    args.task,
                );

                // Send task to subagent and wait for response
                match self.acp.send_message(&subagent_id, message).await {
                    Ok(response) => {
                        // For Run mode, the subagent terminates after completion
                        // For Session mode, the subagent remains available
                        let mode_info = match handle.mode {
                            SpawnMode::Run => "Subagent completed and terminated",
                            SpawnMode::Session => "Subagent remains active in session",
                        };

                        Ok(ToolExecutionResult {
                            success: true,
                            output: format!("{}", response),
                            error: None,
                            data: Some(serde_json::json!({
                                "subagent_id": subagent_id,
                                "session_id": session_id.to_string(),
                                "mode": format!("{:?}", handle.mode),
                                "status": mode_info,
                                "response": response,
                            })),
                            execution_time: start.elapsed(),
                        })
                    }
                    Err(e) => {
                        warn!("Subagent {} failed to process task: {}", subagent_id, e);

                        // Try to shutdown the subagent
                        let _ = self.acp.shutdown_subagent(&subagent_id).await;

                        Ok(ToolExecutionResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!("Subagent failed: {}", e)),
                            data: Some(serde_json::json!({
                                "subagent_id": subagent_id,
                                "error": e.to_string(),
                            })),
                            execution_time: start.elapsed(),
                        })
                    }
                }
            }
            Err(e) => {
                warn!("Failed to spawn subagent: {}", e);
                Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to spawn subagent: {}", e)),
                    data: None,
                    execution_time: start.elapsed(),
                })
            }
        }
    }

    fn is_available(&self, _context: &ToolContext) -> bool {
        // ACP tool is always available if the ACP is enabled
        true
    }
}

/// Tool for managing ACP sessions
pub struct AcpSessionTool {
    acp: Arc<AcpControlPlane>,
}

impl AcpSessionTool {
    /// Create a new ACP session management tool
    pub fn new(acp: Arc<AcpControlPlane>) -> Self {
        Self { acp }
    }
}

/// Arguments for session management
#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum SessionAction {
    /// List active sessions
    List,
    /// Get session info
    Get { session_id: String },
    /// Terminate a session
    Terminate { session_id: String },
    /// Send message to a session subagent
    Message {
        session_id: String,
        subagent_id: String,
        message: String,
    },
}

#[async_trait]
impl Tool for AcpSessionTool {
    fn name(&self) -> &str {
        "manage_acp_session"
    }

    fn description(&self) -> &str {
        "Manage ACP (Agent Control Plane) sessions. List active sessions, get session info, terminate sessions, or send messages to active subagents."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "get", "terminate", "message"],
                    "description": "Action to perform"
                },
                "session_id": {
                    "type": "string",
                    "description": "Session ID (required for get, terminate, message)"
                },
                "subagent_id": {
                    "type": "string",
                    "description": "Subagent ID (required for message action)"
                },
                "message": {
                    "type": "string",
                    "description": "Message to send (required for message action)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();

        let action: SessionAction = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Invalid arguments: {}", e)),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        };

        match action {
            SessionAction::List => {
                // List all subagents as a proxy for sessions
                let subagents = self.acp.list_subagents().await;

                let session_info: Vec<_> = subagents
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "subagent_id": s.id,
                            "session_id": s.session_id.to_string(),
                            "parent_id": s.parent_id,
                            "mode": format!("{:?}", s.mode),
                            "status": format!("{:?}", s.status),
                            "thread_id": s.thread_id,
                        })
                    })
                    .collect();

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Found {} active subagent(s)", subagents.len()),
                    error: None,
                    data: Some(serde_json::json!({ "subagents": session_info })),
                    execution_time: start.elapsed(),
                })
            }
            SessionAction::Get { session_id } => {
                let session_id = AcpSessionId(session_id);

                match self.acp.get_session_info(&session_id).await {
                    Some(info) => Ok(ToolExecutionResult {
                        success: true,
                        output: format!(
                            "Session {} has {} subagent(s)",
                            info.id, info.subagent_count
                        ),
                        error: None,
                        data: Some(serde_json::json!({
                            "id": info.id.to_string(),
                            "parent_agent_id": info.parent_agent_id,
                            "subagent_count": info.subagent_count,
                            "created_at": info.created_at.to_rfc3339(),
                        })),
                        execution_time: start.elapsed(),
                    }),
                    None => Ok(ToolExecutionResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Session {} not found", session_id)),
                        data: None,
                        execution_time: start.elapsed(),
                    }),
                }
            }
            SessionAction::Terminate { session_id } => {
                let session_id = AcpSessionId(session_id);

                match self.acp.terminate_session(&session_id).await {
                    Ok(count) => Ok(ToolExecutionResult {
                        success: true,
                        output: format!(
                            "Terminated {} subagent(s) in session {}",
                            count, session_id
                        ),
                        error: None,
                        data: Some(serde_json::json!({
                            "terminated_count": count,
                            "session_id": session_id.to_string(),
                        })),
                        execution_time: start.elapsed(),
                    }),
                    Err(e) => Ok(ToolExecutionResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to terminate session: {}", e)),
                        data: None,
                        execution_time: start.elapsed(),
                    }),
                }
            }
            SessionAction::Message {
                session_id,
                subagent_id,
                message,
            } => {
                let _session_id = AcpSessionId(session_id);
                let incoming = IncomingMessage::new(
                    "user".to_string(),
                    "tool-invocation".to_string(),
                    message,
                );

                match self.acp.send_message(&subagent_id, incoming).await {
                    Ok(response) => Ok(ToolExecutionResult {
                        success: true,
                        output: response.clone(),
                        error: None,
                        data: Some(serde_json::json!({
                            "subagent_id": subagent_id,
                            "response": response,
                        })),
                        execution_time: start.elapsed(),
                    }),
                    Err(e) => Ok(ToolExecutionResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to send message: {}", e)),
                        data: None,
                        execution_time: start.elapsed(),
                    }),
                }
            }
        }
    }
}
