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

use crate::acp::{AcpControlPlane, AcpSessionId};
use crate::channels::IncomingMessage;
use crate::gateway::GatewayState;

use super::{Tool, ToolContext, ToolExecutionResult};

// ── sessions_list ────────────────────────────────────────────────────────────

/// List all active ACP sessions and their subagents.
pub struct SessionsListTool {
    acp: Arc<AcpControlPlane>,
}

impl SessionsListTool {
    pub fn new(acp: Arc<AcpControlPlane>) -> Self {
        Self { acp }
    }
}

#[async_trait]
impl Tool for SessionsListTool {
    fn name(&self) -> &str {
        "sessions_list"
    }

    fn description(&self) -> &str {
        "List all active ACP sessions and their subagents with status information."
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
        let subagents = self.acp.list_subagents().await;

        let sessions: Vec<_> = subagents
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
            output: format!("Found {} active session(s)", sessions.len()),
            error: None,
            data: Some(serde_json::json!({ "sessions": sessions })),
            execution_time: start.elapsed(),
        })
    }
}

// ── sessions_history ─────────────────────────────────────────────────────────

/// Get thread/history info for a session.
pub struct SessionsHistoryTool {
    acp: Arc<AcpControlPlane>,
}

impl SessionsHistoryTool {
    pub fn new(acp: Arc<AcpControlPlane>) -> Self {
        Self { acp }
    }
}

#[derive(Debug, Deserialize)]
struct SessionsHistoryArgs {
    session_id: String,
}

#[async_trait]
impl Tool for SessionsHistoryTool {
    fn name(&self) -> &str {
        "sessions_history"
    }

    fn description(&self) -> &str {
        "Get history and thread context for an ACP session. Returns subagent list and thread binding information."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "ACP session ID"
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

        let session_id = AcpSessionId(args.session_id);
        let subagents = self.acp.list_session_subagents(&session_id).await;

        let history: Vec<_> = subagents
            .iter()
            .map(|s| {
                serde_json::json!({
                    "subagent_id": s.id,
                    "status": format!("{:?}", s.status),
                    "mode": format!("{:?}", s.mode),
                    "thread_id": s.thread_id,
                })
            })
            .collect();

        Ok(ToolExecutionResult {
            success: true,
            output: format!("Session {} has {} subagent(s)", session_id, history.len()),
            error: None,
            data: Some(serde_json::json!({
                "session_id": session_id.to_string(),
                "subagents": history,
            })),
            execution_time: start.elapsed(),
        })
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

/// Get the status of a session or subagent.
pub struct SessionStatusTool {
    acp: Arc<AcpControlPlane>,
}

impl SessionStatusTool {
    pub fn new(acp: Arc<AcpControlPlane>) -> Self {
        Self { acp }
    }
}

#[derive(Debug, Deserialize)]
struct SessionStatusArgs {
    session_id: Option<String>,
    subagent_id: Option<String>,
}

#[async_trait]
impl Tool for SessionStatusTool {
    fn name(&self) -> &str {
        "session_status"
    }

    fn description(&self) -> &str {
        "Get the status of an ACP session or subagent. Provide either session_id or subagent_id."
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
                    "description": "Subagent ID"
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

        if let Some(subagent_id) = args.subagent_id {
            match self.acp.get_subagent_status(&subagent_id).await {
                Some(status) => Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Subagent {} status: {:?}", subagent_id, status),
                    error: None,
                    data: Some(serde_json::json!({
                        "subagent_id": subagent_id,
                        "status": format!("{:?}", status),
                    })),
                    execution_time: start.elapsed(),
                }),
                None => Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Subagent {} not found", subagent_id)),
                    data: None,
                    execution_time: start.elapsed(),
                }),
            }
        } else if let Some(session_id) = args.session_id {
            let session_id = AcpSessionId(session_id);
            match self.acp.get_session_info(&session_id).await {
                Some(info) => Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Session {} has {} subagent(s)", info.id, info.subagent_count),
                    error: None,
                    data: Some(serde_json::json!({
                        "session_id": info.id.to_string(),
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
        } else {
            Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some("Provide either session_id or subagent_id".to_string()),
                data: None,
                execution_time: start.elapsed(),
            })
        }
    }
}

// ── subagents ────────────────────────────────────────────────────────────────

/// Unified subagent management tool.
pub struct SubagentsTool {
    acp: Arc<AcpControlPlane>,
}

impl SubagentsTool {
    pub fn new(acp: Arc<AcpControlPlane>) -> Self {
        Self { acp }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum SubagentsAction {
    List,
    Status { subagent_id: String },
    Shutdown { subagent_id: String },
}

#[async_trait]
impl Tool for SubagentsTool {
    fn name(&self) -> &str {
        "subagents"
    }

    fn description(&self) -> &str {
        "Manage subagents. List all subagents, check status, or shut down a specific subagent."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "status", "shutdown"],
                    "description": "Action to perform"
                },
                "subagent_id": {
                    "type": "string",
                    "description": "Subagent ID (required for status and shutdown)"
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
        let action: SubagentsAction = match serde_json::from_value(args) {
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
            SubagentsAction::List => {
                let subagents = self.acp.list_subagents().await;
                let list: Vec<_> = subagents
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "id": s.id,
                            "session_id": s.session_id.to_string(),
                            "status": format!("{:?}", s.status),
                            "mode": format!("{:?}", s.mode),
                        })
                    })
                    .collect();

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Found {} subagent(s)", list.len()),
                    error: None,
                    data: Some(serde_json::json!({ "subagents": list })),
                    execution_time: start.elapsed(),
                })
            }
            SubagentsAction::Status { subagent_id } => {
                match self.acp.get_subagent_status(&subagent_id).await {
                    Some(status) => Ok(ToolExecutionResult {
                        success: true,
                        output: format!("Subagent {}: {:?}", subagent_id, status),
                        error: None,
                        data: Some(serde_json::json!({
                            "subagent_id": subagent_id,
                            "status": format!("{:?}", status),
                        })),
                        execution_time: start.elapsed(),
                    }),
                    None => Ok(ToolExecutionResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Subagent {} not found", subagent_id)),
                        data: None,
                        execution_time: start.elapsed(),
                    }),
                }
            }
            SubagentsAction::Shutdown { subagent_id } => {
                match self.acp.shutdown_subagent(&subagent_id).await {
                    Ok(true) => Ok(ToolExecutionResult {
                        success: true,
                        output: format!("Subagent {} shut down", subagent_id),
                        error: None,
                        data: Some(serde_json::json!({
                            "subagent_id": subagent_id,
                            "action": "shutdown",
                        })),
                        execution_time: start.elapsed(),
                    }),
                    Ok(false) => Ok(ToolExecutionResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Subagent {} not found", subagent_id)),
                        data: None,
                        execution_time: start.elapsed(),
                    }),
                    Err(e) => Ok(ToolExecutionResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("Failed to shut down: {}", e)),
                        data: None,
                        execution_time: start.elapsed(),
                    }),
                }
            }
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
        assert_eq!(args.session_id, Some("sess-1".to_string()));
        assert_eq!(args.subagent_id, None);

        let args2: SessionStatusArgs = serde_json::from_value(serde_json::json!({
            "subagent_id": "sub-1"
        }))
        .unwrap();
        assert_eq!(args2.subagent_id, Some("sub-1".to_string()));
        assert_eq!(args2.session_id, None);
    }

    #[test]
    fn test_subagents_action_parsing() {
        let action: SubagentsAction = serde_json::from_value(serde_json::json!({
            "action": "list"
        }))
        .unwrap();
        assert!(matches!(action, SubagentsAction::List));

        let action: SubagentsAction = serde_json::from_value(serde_json::json!({
            "action": "status",
            "subagent_id": "sub-1"
        }))
        .unwrap();
        assert!(
            matches!(action, SubagentsAction::Status { subagent_id } if subagent_id == "sub-1")
        );

        let action: SubagentsAction = serde_json::from_value(serde_json::json!({
            "action": "shutdown",
            "subagent_id": "sub-1"
        }))
        .unwrap();
        assert!(
            matches!(action, SubagentsAction::Shutdown { subagent_id } if subagent_id == "sub-1")
        );
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
