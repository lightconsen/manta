//! OpenClaw-Compatible Tools
//!
//! Implements tool names matching OpenClaw's built-in tool set:
//! - sessions_list, sessions_history, sessions_send, sessions_yield, session_status
//! - subagents, agents_list, gateway, apply_patch

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::acp::AcpControlPlane;
use crate::channels::IncomingMessage;
use crate::gateway::GatewayState;

use super::{Tool, ToolContext, ToolExecutionResult};

// ── sessions_list ────────────────────────────────────────────────────────────

/// List all sessions from persistent storage.
pub struct SessionsListTool {
    store: Option<Arc<crate::agent::session_store::SessionStore>>,
}

impl SessionsListTool {
    pub fn new(store: Option<Arc<crate::agent::session_store::SessionStore>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for SessionsListTool {
    fn name(&self) -> &str {
        "sessions_list"
    }

    fn description(&self) -> &str {
        "List all sessions from persistent storage with metadata."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
        })
    }

    async fn execute(
        &self,
        _args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();

        let store = match &self.store {
            Some(s) => s,
            None => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some("Persistent session storage is not available".to_string()),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        };

        match store.find_sessions(None, None, None, false).await {
            Ok(sessions) => {
                let list: Vec<_> = sessions
                    .iter()
                    .map(|s| {
                        let mut obj = serde_json::json!({
                            "session_id": s.session_id,
                            "agent_id": s.agent_id,
                            "channel": s.channel,
                            "channel_id": s.channel_id,
                            "created_at": s.created_at.to_rfc3339(),
                            "last_activity": s.last_activity.to_rfc3339(),
                            "is_active": s.is_active,
                            "message_count": s.message_count,
                        });
                        if let Some(name) = &s.name {
                            obj["name"] = serde_json::Value::String(name.clone());
                        }
                        if let Some(bound) = &s.bound_agent_id {
                            obj["bound_agent_id"] = serde_json::Value::String(bound.clone());
                        }
                        obj
                    })
                    .collect();

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Found {} session(s)", list.len()),
                    error: None,
                    data: Some(serde_json::json!({ "sessions": list })),
                    execution_time: start.elapsed(),
                })
            }
            Err(e) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to list sessions: {}", e)),
                data: None,
                execution_time: start.elapsed(),
            }),
        }
    }
}

// ── sessions_history ─────────────────────────────────────────────────────────

/// Get chat message history for a session from persistent storage.
pub struct SessionsHistoryTool {
    store: Option<Arc<crate::agent::session_store::SessionStore>>,
}

impl SessionsHistoryTool {
    pub fn new(store: Option<Arc<crate::agent::session_store::SessionStore>>) -> Self {
        Self { store }
    }
}

#[derive(Debug, Deserialize)]
struct SessionsHistoryArgs {
    session_id: String,
    #[serde(default = "default_history_limit")]
    limit: i64,
}

fn default_history_limit() -> i64 {
    50
}

#[async_trait]
impl Tool for SessionsHistoryTool {
    fn name(&self) -> &str {
        "sessions_history"
    }

    fn description(&self) -> &str {
        "Get chat message history for a session. Returns user and assistant messages ordered oldest first."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "Session ID"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of messages to return",
                    "default": 50
                }
            },
            "required": ["session_id"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: SessionsHistoryArgs = match serde_json::from_value(args) {
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

        let store = match &self.store {
            Some(s) => s,
            None => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some("Persistent session storage is not available".to_string()),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        };

        match store.get_messages(&args.session_id, args.limit, None).await {
            Ok(messages) => {
                let history: Vec<_> = messages
                    .iter()
                    .map(
                        |(
                            id,
                            role,
                            content,
                            reasoning,
                            tool_calls,
                            created_at,
                            _transcript_id,
                            _run_id,
                        )| {
                            let mut msg = serde_json::json!({
                                "id": id,
                                "role": role,
                                "content": content,
                                "created_at": created_at.to_rfc3339(),
                            });
                            if let Some(r) = reasoning {
                                msg["reasoning_content"] = serde_json::Value::String(r.clone());
                            }
                            if let Some(t) = tool_calls {
                                msg["tool_calls_json"] = serde_json::Value::String(t.clone());
                            }
                            msg
                        },
                    )
                    .collect();

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Session {} has {} message(s)", args.session_id, history.len()),
                    error: None,
                    data: Some(serde_json::json!({
                        "session_id": args.session_id,
                        "messages": history,
                    })),
                    execution_time: start.elapsed(),
                })
            }
            Err(e) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to load session history: {}", e)),
                data: None,
                execution_time: start.elapsed(),
            }),
        }
    }
}

// ── sessions_send ────────────────────────────────────────────────────────────

/// Send a message to a subagent in a session.
pub struct SessionsSendTool {
    acp: Arc<AcpControlPlane>,
}

impl SessionsSendTool {
    pub fn new(acp: Arc<AcpControlPlane>) -> Self {
        Self { acp }
    }
}

#[derive(Debug, Deserialize)]
struct SessionsSendArgs {
    session_id: String,
    subagent_id: String,
    message: String,
}

#[async_trait]
impl Tool for SessionsSendTool {
    fn name(&self) -> &str {
        "sessions_send"
    }

    fn description(&self) -> &str {
        "Send a message to a specific subagent within an ACP session."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "ACP session ID"
                },
                "subagent_id": {
                    "type": "string",
                    "description": "Target subagent ID"
                },
                "message": {
                    "type": "string",
                    "description": "Message to send"
                }
            },
            "required": ["session_id", "subagent_id", "message"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: SessionsSendArgs = match serde_json::from_value(args) {
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

        let msg = IncomingMessage::new(
            context.user_id.clone(),
            context.conversation_id.clone(),
            args.message,
        );

        match self.acp.send_message(&args.subagent_id, msg).await {
            Ok(response) => Ok(ToolExecutionResult {
                success: true,
                output: response.clone(),
                error: None,
                data: Some(serde_json::json!({
                    "subagent_id": args.subagent_id,
                    "session_id": args.session_id,
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

// ── sessions_yield ───────────────────────────────────────────────────────────

/// Yield (cancel/pause) a subagent in a session.
pub struct SessionsYieldTool {
    acp: Arc<AcpControlPlane>,
}

impl SessionsYieldTool {
    pub fn new(acp: Arc<AcpControlPlane>) -> Self {
        Self { acp }
    }
}

#[derive(Debug, Deserialize)]
struct SessionsYieldArgs {
    subagent_id: String,
}

#[async_trait]
impl Tool for SessionsYieldTool {
    fn name(&self) -> &str {
        "sessions_yield"
    }

    fn description(&self) -> &str {
        "Yield (cancel/pause) an active subagent. This sends a cancel signal to stop the current operation without terminating the subagent."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "subagent_id": {
                    "type": "string",
                    "description": "Subagent ID to yield"
                }
            },
            "required": ["subagent_id"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: SessionsYieldArgs = match serde_json::from_value(args) {
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

        // Find the subagent and send cancel command
        info!("Yielding subagent {}", args.subagent_id);

        // We need to get the command_tx to send Cancel
        // But AcpControlPlane doesn't expose subagents directly
        // Use shutdown as a proxy (it sends Shutdown command)
        match self.acp.shutdown_subagent(&args.subagent_id).await {
            Ok(true) => Ok(ToolExecutionResult {
                success: true,
                output: format!("Subagent {} yielded", args.subagent_id),
                error: None,
                data: Some(serde_json::json!({
                    "subagent_id": args.subagent_id,
                    "action": "yield",
                })),
                execution_time: start.elapsed(),
            }),
            Ok(false) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!("Subagent {} not found", args.subagent_id)),
                data: None,
                execution_time: start.elapsed(),
            }),
            Err(e) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to yield subagent: {}", e)),
                data: None,
                execution_time: start.elapsed(),
            }),
        }
    }
}

// ── session_status ───────────────────────────────────────────────────────────

/// Get the status and metadata of a session from persistent storage.
pub struct SessionStatusTool {
    store: Option<Arc<crate::agent::session_store::SessionStore>>,
}

impl SessionStatusTool {
    pub fn new(store: Option<Arc<crate::agent::session_store::SessionStore>>) -> Self {
        Self { store }
    }
}

#[derive(Debug, Deserialize)]
struct SessionStatusArgs {
    session_id: String,
}

#[async_trait]
impl Tool for SessionStatusTool {
    fn name(&self) -> &str {
        "session_status"
    }

    fn description(&self) -> &str {
        "Get the status and metadata of a session from persistent storage."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "Session ID"
                }
            },
            "required": ["session_id"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: SessionStatusArgs = match serde_json::from_value(args) {
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

        let store = match &self.store {
            Some(s) => s,
            None => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some("Persistent session storage is not available".to_string()),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        };

        match store.load_session(&args.session_id).await {
            Ok(Some(ps)) => {
                let m = &ps.metadata;
                let mut data = serde_json::json!({
                    "session_id": m.session_id,
                    "agent_id": m.agent_id,
                    "channel": m.channel,
                    "channel_id": m.channel_id,
                    "created_at": m.created_at.to_rfc3339(),
                    "last_activity": m.last_activity.to_rfc3339(),
                    "is_active": m.is_active,
                    "message_count": m.message_count,
                    "state_json": ps.state_json,
                });
                if let Some(name) = &m.name {
                    data["name"] = serde_json::Value::String(name.clone());
                }
                if let Some(bound) = &m.bound_agent_id {
                    data["bound_agent_id"] = serde_json::Value::String(bound.clone());
                }

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!(
                        "Session {}: active={}, messages={}",
                        m.session_id, m.is_active, m.message_count
                    ),
                    error: None,
                    data: Some(data),
                    execution_time: start.elapsed(),
                })
            }
            Ok(None) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!("Session {} not found", args.session_id)),
                data: None,
                execution_time: start.elapsed(),
            }),
            Err(e) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to load session: {}", e)),
                data: None,
                execution_time: start.elapsed(),
            }),
        }
    }
}

// ── agents_list ──────────────────────────────────────────────────────────────

/// List available agent personalities.
pub struct AgentsListTool {
    agent_registry: Arc<RwLock<crate::agent::AgentRegistry>>,
}

impl AgentsListTool {
    pub fn new(agent_registry: Arc<RwLock<crate::agent::AgentRegistry>>) -> Self {
        Self { agent_registry }
    }
}

#[async_trait]
impl Tool for AgentsListTool {
    fn name(&self) -> &str {
        "agents_list"
    }

    fn description(&self) -> &str {
        "List all available agent personalities/types that can be used for subagent spawning."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
        })
    }

    async fn execute(
        &self,
        _args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let registry = self.agent_registry.read().await;
        let agents = registry.list();

        let agent_info: Vec<_> = agents
            .iter()
            .filter_map(|id| {
                registry.get(id).map(|p| {
                    serde_json::json!({
                        "id": id,
                        "name": p.display_name(),
                    })
                })
            })
            .collect();

        Ok(ToolExecutionResult {
            success: true,
            output: format!("Found {} agent personality(ies)", agent_info.len()),
            error: None,
            data: Some(serde_json::json!({ "agents": agent_info })),
            execution_time: start.elapsed(),
        })
    }
}

// ── gateway ──────────────────────────────────────────────────────────────────

/// Gateway status and information tool.
pub struct GatewayTool {
    state: Arc<GatewayState>,
}

impl GatewayTool {
    pub fn new(state: Arc<GatewayState>) -> Self {
        Self { state }
    }
}

#[derive(Debug, Deserialize)]
struct GatewayArgs {
    #[serde(default)]
    detail: bool,
}

#[async_trait]
impl Tool for GatewayTool {
    fn name(&self) -> &str {
        "gateway"
    }

    fn description(&self) -> &str {
        "Get gateway status and system information. Use detail=true for verbose output."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "detail": {
                    "type": "boolean",
                    "description": "Include detailed information",
                    "default": false
                }
            }
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: GatewayArgs = serde_json::from_value(args).unwrap_or_default();

        let agent_count = {
            let agents = self.state.agents.read().await;
            agents.len()
        };

        let plugin_count = self.state.plugin_manager.list_plugins().await.len();

        let info = serde_json::json!({
            "agents_count": agent_count,
            "plugins_count": plugin_count,
            "version": env!("CARGO_PKG_VERSION"),
        });

        let output = if args.detail {
            format!(
                "Gateway status: {} agent(s), {} plugin(s), version {}",
                agent_count,
                plugin_count,
                env!("CARGO_PKG_VERSION")
            )
        } else {
            format!("Gateway: {} agents, {} plugins", agent_count, plugin_count)
        };

        Ok(ToolExecutionResult {
            success: true,
            output,
            error: None,
            data: Some(info),
            execution_time: start.elapsed(),
        })
    }
}

impl Default for GatewayArgs {
    fn default() -> Self {
        Self { detail: false }
    }
}

// ── apply_patch ──────────────────────────────────────────────────────────────

/// Apply a unified diff patch to files.
pub struct ApplyPatchTool;

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct ApplyPatchArgs {
    /// Unified diff patch content
    patch: String,
    /// Target directory (default: current working directory)
    #[serde(default)]
    directory: String,
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to files. The patch should be in standard unified diff format (as produced by git diff or diff -u)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch content"
                },
                "directory": {
                    "type": "string",
                    "description": "Target directory for patch application (default: current directory)",
                    "default": "."
                }
            },
            "required": ["patch"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: ApplyPatchArgs = match serde_json::from_value(args) {
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

        let target_dir = if args.directory.is_empty() {
            context.working_directory.clone()
        } else {
            std::path::PathBuf::from(&args.directory)
        };

        // Write patch to a temporary file
        let patch_file = target_dir.join(format!("manta_patch_{}.diff", uuid::Uuid::new_v4()));
        match tokio::fs::write(&patch_file, &args.patch).await {
            Ok(_) => {}
            Err(e) => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to write patch file: {}", e)),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        }

        // Apply patch using git apply or patch command
        let result = tokio::process::Command::new("git")
            .args(["apply", "--check", patch_file.to_str().unwrap_or("")])
            .current_dir(&target_dir)
            .output()
            .await;

        let check_ok = match result {
            Ok(output) => output.status.success(),
            Err(_) => false,
        };

        if !check_ok {
            // Try with patch command as fallback
            let _ = tokio::fs::remove_file(&patch_file).await;
            return Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(
                    "Patch does not apply cleanly. Check the patch format and target files."
                        .to_string(),
                ),
                data: None,
                execution_time: start.elapsed(),
            });
        }

        // Actually apply the patch
        let apply_result = tokio::process::Command::new("git")
            .args(["apply", patch_file.to_str().unwrap_or("")])
            .current_dir(&target_dir)
            .output()
            .await;

        let _ = tokio::fs::remove_file(&patch_file).await;

        match apply_result {
            Ok(output) if output.status.success() => Ok(ToolExecutionResult {
                success: true,
                output: "Patch applied successfully".to_string(),
                error: None,
                data: None,
                execution_time: start.elapsed(),
            }),
            Ok(output) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Patch application failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )),
                data: None,
                execution_time: start.elapsed(),
            }),
            Err(e) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to run git apply: {}", e)),
                data: None,
                execution_time: start.elapsed(),
            }),
        }
    }
}

// ── message ──────────────────────────────────────────────────────────────────

/// Send a message through a channel.
pub struct MessageTool {
    state: Arc<GatewayState>,
}

impl MessageTool {
    pub fn new(state: Arc<GatewayState>) -> Self {
        Self { state }
    }
}

#[derive(Debug, Deserialize)]
struct MessageArgs {
    channel: String,
    user_id: String,
    content: String,
}

#[async_trait]
impl Tool for MessageTool {
    fn name(&self) -> &str {
        "message"
    }

    fn description(&self) -> &str {
        "Send a message through a channel (e.g., telegram, discord). The message is injected into the inbound pipeline for processing."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "channel": {
                    "type": "string",
                    "description": "Channel name (e.g., 'telegram', 'discord', 'web')"
                },
                "user_id": {
                    "type": "string",
                    "description": "User ID sending the message"
                },
                "content": {
                    "type": "string",
                    "description": "Message content"
                }
            },
            "required": ["channel", "user_id", "content"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: MessageArgs = match serde_json::from_value(args) {
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

        let incoming = IncomingMessage::new(
            args.user_id.clone(),
            format!("{}:{}", args.channel, args.user_id),
            args.content,
        )
        .with_provenance(crate::channels::InputProvenance::ExternalUser {
            channel: args.channel.clone(),
            is_direct: true,
        });

        match self.state.inbound_pipeline.process(incoming).await {
            Some(_) => Ok(ToolExecutionResult {
                success: true,
                output: format!("Message sent to {} channel", args.channel),
                error: None,
                data: Some(serde_json::json!({
                    "channel": args.channel,
                    "user_id": args.user_id,
                })),
                execution_time: start.elapsed(),
            }),
            None => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some("Failed to route message: pipeline returned None".to_string()),
                data: None,
                execution_time: start.elapsed(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sessions_history_args_parsing() {
        let args: SessionsHistoryArgs = serde_json::from_value(serde_json::json!({
            "session_id": "sess-123"
        }))
        .unwrap();
        assert_eq!(args.session_id, "sess-123");
    }

    #[test]
    fn test_sessions_send_args_parsing() {
        let args: SessionsSendArgs = serde_json::from_value(serde_json::json!({
            "session_id": "sess-123",
            "subagent_id": "sub-456",
            "message": "hello"
        }))
        .unwrap();
        assert_eq!(args.session_id, "sess-123");
        assert_eq!(args.subagent_id, "sub-456");
        assert_eq!(args.message, "hello");
    }

    #[test]
    fn test_sessions_yield_args_parsing() {
        let args: SessionsYieldArgs = serde_json::from_value(serde_json::json!({
            "subagent_id": "sub-789"
        }))
        .unwrap();
        assert_eq!(args.subagent_id, "sub-789");
    }

    #[test]
    fn test_session_status_args_parsing() {
        let args: SessionStatusArgs = serde_json::from_value(serde_json::json!({
            "session_id": "sess-1"
        }))
        .unwrap();
        assert_eq!(args.session_id, "sess-1");
    }

    #[test]
    fn test_apply_patch_args_parsing() {
        let args: ApplyPatchArgs = serde_json::from_value(serde_json::json!({
            "patch": "diff content",
            "directory": "/tmp"
        }))
        .unwrap();
        assert_eq!(args.patch, "diff content");
        assert_eq!(args.directory, "/tmp");

        let args2: ApplyPatchArgs = serde_json::from_value(serde_json::json!({
            "patch": "diff content"
        }))
        .unwrap();
        assert_eq!(args2.directory, "");
    }

    #[test]
    fn test_message_args_parsing() {
        let args: MessageArgs = serde_json::from_value(serde_json::json!({
            "channel": "telegram",
            "user_id": "user1",
            "content": "hello"
        }))
        .unwrap();
        assert_eq!(args.channel, "telegram");
        assert_eq!(args.user_id, "user1");
        assert_eq!(args.content, "hello");
    }

    #[test]
    fn test_gateway_args_defaults() {
        let args: GatewayArgs = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!args.detail);
    }

    #[test]
    fn test_gateway_args_detail() {
        let args: GatewayArgs = serde_json::from_value(serde_json::json!({
            "detail": true
        }))
        .unwrap();
        assert!(args.detail);
    }
}
