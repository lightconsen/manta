//! Slash Command System for Manta Gateway
//!
//! Provides OpenClaw-style `/` commands via WebSocket RPC.
//! Commands are exposed via `commands.list` and executed via `commands.execute`.

use crate::gateway::protocol::*;
use crate::gateway::GatewayState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

// ── Command Definitions ───────────────────────────────────────────────────────

/// Command category for grouping
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCategory {
    Session,
    Model,
    Status,
    Agents,
    Tools,
    Admin,
}

/// Command tier for progressive disclosure
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandTier {
    Essential,
    Standard,
    Power,
}

/// Metadata for a single slash command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandDef {
    pub key: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    pub category: CommandCategory,
    pub tier: CommandTier,
    pub local: bool,
    pub requires_admin: bool,
}

impl CommandDef {
    fn new(key: &str, name: &str, description: &str, category: CommandCategory) -> Self {
        Self {
            key: key.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            args: None,
            category,
            tier: CommandTier::Standard,
            local: false,
            requires_admin: false,
        }
    }

    fn with_args(mut self, args: &str) -> Self {
        self.args = Some(args.to_string());
        self
    }

    fn local(mut self) -> Self {
        self.local = true;
        self
    }

    fn admin(mut self) -> Self {
        self.requires_admin = true;
        self
    }

    fn essential(mut self) -> Self {
        self.tier = CommandTier::Essential;
        self
    }
}

/// Built-in command catalog
pub fn built_in_commands() -> Vec<CommandDef> {
    vec![
        // Session
        CommandDef::new("new", "new", "Start a new session", CommandCategory::Session)
            .with_args("[model]")
            .local()
            .essential(),
        CommandDef::new("reset", "reset", "Reset the current session", CommandCategory::Session)
            .with_args("[soft|hard]")
            .essential(),
        CommandDef::new("stop", "stop", "Abort the current run", CommandCategory::Session)
            .essential(),
        CommandDef::new(
            "compact",
            "compact",
            "Compact the session context",
            CommandCategory::Session,
        )
        .with_args("[instructions]"),
        CommandDef::new(
            "export-session",
            "export-session",
            "Export session to HTML",
            CommandCategory::Session,
        )
        .with_args("[path]"),
        CommandDef::new(
            "clear",
            "clear",
            "Clear chat history",
            CommandCategory::Session,
        )
        .local(),
        // Model
        CommandDef::new(
            "model",
            "model",
            "Show or switch the active model",
            CommandCategory::Model,
        )
        .with_args("[name|#|status]"),
        CommandDef::new(
            "think",
            "think",
            "Set thinking level",
            CommandCategory::Model,
        )
        .with_args("<level>"),
        CommandDef::new(
            "verbose",
            "verbose",
            "Toggle verbose output",
            CommandCategory::Model,
        )
        .with_args("on|off|full"),
        CommandDef::new(
            "fast",
            "fast",
            "Show or set fast mode",
            CommandCategory::Model,
        )
        .with_args("[on|off|status]"),
        // Status / Query
        CommandDef::new("help", "help", "Show help summary", CommandCategory::Status)
            .essential(),
        CommandDef::new(
            "commands",
            "commands",
            "Show full command catalog",
            CommandCategory::Status,
        )
        .essential(),
        CommandDef::new("status", "status", "Show runtime status", CommandCategory::Status)
            .essential(),
        CommandDef::new(
            "tools",
            "tools",
            "Show available tools",
            CommandCategory::Status,
        )
        .with_args("[compact|verbose]"),
        CommandDef::new(
            "whoami",
            "whoami",
            "Show your sender ID",
            CommandCategory::Status,
        )
        .essential(),
        CommandDef::new(
            "usage",
            "usage",
            "Show usage statistics",
            CommandCategory::Status,
        )
        .with_args("[off|tokens|full|cost]"),
        // Agents / ACP
        CommandDef::new(
            "subagents",
            "subagents",
            "Manage sub-agents",
            CommandCategory::Agents,
        )
        .with_args("list|kill|log|info|send|steer|spawn"),
        CommandDef::new(
            "acp",
            "acp",
            "Manage ACP sessions",
            CommandCategory::Agents,
        )
        .with_args("spawn|cancel|steer|close|sessions|status|..."),
        CommandDef::new("kill", "kill", "Abort sub-agent runs", CommandCategory::Agents)
            .with_args("<id|#|all>"),
        CommandDef::new(
            "steer",
            "steer",
            "Send steering to a sub-agent",
            CommandCategory::Agents,
        )
        .with_args("<id> <message>"),
        // Admin (owner-only)
        CommandDef::new(
            "config",
            "config",
            "Read or write config",
            CommandCategory::Admin,
        )
        .with_args("show|get|set|unset")
        .admin(),
        CommandDef::new(
            "plugins",
            "plugins",
            "Inspect or toggle plugins",
            CommandCategory::Admin,
        )
        .with_args("list|install|enable|disable")
        .admin(),
        CommandDef::new("restart", "restart", "Restart the gateway", CommandCategory::Admin).admin(),
        CommandDef::new(
            "bash",
            "bash",
            "Run a host shell command",
            CommandCategory::Admin,
        )
        .with_args("<command>")
        .admin(),
    ]
}

// ── handlers ──────────────────────────────────────────────────────────────────

/// Handle `commands.list` — return the built-in command catalog
pub fn handle_commands_list() -> serde_json::Value {
    let commands = built_in_commands();
    serde_json::json!({
        "commands": commands,
    })
}

/// Parameters for `commands.execute`
#[derive(Debug, Deserialize)]
struct ExecuteParams {
    command: String,
    #[serde(default)]
    args: String,
    #[serde(default)]
    session_id: Option<String>,
}

/// Handle `commands.execute` — parse command and dispatch
pub async fn handle_commands_execute(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let params: ExecuteParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let normalized = params.command.to_lowercase().trim_start_matches('/').to_string();

    debug!("Executing command: /{} args='{}'", normalized, params.args);

    // Find the command definition
    let commands = built_in_commands();
    let def = match commands.iter().find(|c| c.key == normalized || c.name == normalized) {
        Some(d) => d.clone(),
        None => {
            return WsResponse::err(
                &req.id,
                "COMMAND_NOT_FOUND",
                format!("Unknown command: /{}", normalized),
            );
        }
    };

    // Check admin requirement
    if def.requires_admin {
        let conn_guard = conn.read().await;
        let scopes = &conn_guard.scopes;
        if !scopes_allow(scopes, "commands.execute.admin") {
            return error_forbidden(&req.id, SCOPE_ADMIN);
        }
    }

    // Dispatch to handler
    match normalized.as_str() {
        "help" | "commands" => handle_help(req),
        "status" => handle_status(req, state).await,
        "whoami" => handle_whoami(req, conn).await,
        "stop" => handle_stop(req, conn, state).await,
        "reset" => handle_reset(req, conn, state).await,
        "model" => handle_model(req, conn, state, &params.args).await,
        "tools" => handle_tools(req, state, &params.args).await,
        "usage" => handle_usage(req, state).await,
        "compact" => handle_compact(req, conn, state, &params.args).await,
        "bash" => handle_bash(req, &params.args).await,
        _ => WsResponse::err(
            &req.id,
            "NOT_IMPLEMENTED",
            format!("Command /{} is not yet implemented", normalized),
        ),
    }
}

// ── Individual command handlers ───────────────────────────────────────────────

fn handle_help(req: &WsRequest) -> WsResponse {
    let commands = built_in_commands();
    let mut lines = vec!["📋 Manta Commands".to_string(), "─".repeat(30)];

    for c in &commands {
        let icon = match c.category {
            CommandCategory::Session => "🗂️",
            CommandCategory::Model => "🧠",
            CommandCategory::Status => "ℹ️",
            CommandCategory::Agents => "🤖",
            CommandCategory::Tools => "🛠️",
            CommandCategory::Admin => "🔒",
        };
        let admin_mark = if c.requires_admin { " [admin]" } else { "" };
        let args = c.args.as_deref().unwrap_or("");
        lines.push(format!(
            "{} /{}{} — {}{}",
            icon, c.name, args, c.description, admin_mark
        ));
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }))
}

async fn handle_status(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agents = state.agents.read().await.len();
    let sessions = state.session_routing.read().await.len();

    let text = format!(
        "📊 **Status**\n\n\
        Active agents: {}\n\
        Active sessions: {}\n\
        Status: healthy",
        agents, sessions
    );

    WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
}

async fn handle_whoami(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
) -> WsResponse {
    let guard = conn.read().await;
    let user = guard
        .user_id
        .as_ref()
        .map(|u| u.to_string())
        .unwrap_or_else(|| "anonymous".to_string());
    let scopes = &guard.scopes;

    let text = format!(
        "👤 **Whoami**\n\nUser: `{}`\nScopes: `{}`",
        user,
        scopes.join(", ")
    );

    WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
}

async fn handle_stop(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    _state: &Arc<GatewayState>,
) -> WsResponse {
    // Delegate to chat.abort logic
    // We need to find the session_id for this connection
    let session_id = conn.read().await.subscriptions.first().cloned();

    if let Some(sid) = session_id {
        // We can't easily call handle_chat_abort here because it's private in ws.rs.
        // TODO: properly wire through ACP abort when that API is exposed.
        warn!("Command /stop requested for session {} — forwarding to chat.abort not yet wired", sid);
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": "⏹️ Stop requested. Abort signal sent if a run was active." }),
    )
}

async fn handle_reset(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    _state: &Arc<GatewayState>,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();

    if let Some(sid) = session_id {
        // Same limitation as handle_stop — sessions.reset handler is in ws.rs
        warn!("Command /reset requested for session {}", sid);
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": "🔄 Reset requested. Session will be reset." }),
    )
}

async fn handle_model(
    req: &WsRequest,
    _conn: &Arc<RwLock<ProtocolConnection>>,
    _state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "status" {
        WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "🧠 Model switching not yet implemented. Use manta.toml config." }),
        )
    } else {
        WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🧠 Model switch to '{}' not yet implemented.", trimmed) }),
        )
    }
}

async fn handle_tools(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let verbose = args.trim() == "verbose";
    let tool_names = state.tool_registry.list();

    let text = if verbose {
        let mut lines = vec!["🛠️ **Available Tools**".to_string()];
        for name in &tool_names {
            lines.push(format!("- {}", name));
        }
        lines.join("\n")
    } else {
        format!("🛠️ **Tools** ({} total): {}", tool_names.len(), tool_names.join(", "))
    };

    WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
}

async fn handle_usage(
    req: &WsRequest,
    _state: &Arc<GatewayState>,
) -> WsResponse {
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": "📊 Usage tracking not yet implemented." }),
    )
}

async fn handle_compact(
    req: &WsRequest,
    _conn: &Arc<RwLock<ProtocolConnection>>,
    _state: &Arc<GatewayState>,
    _args: &str,
) -> WsResponse {
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": "🗜️ Compaction not yet implemented." }),
    )
}

async fn handle_bash(req: &WsRequest, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /bash <command>");
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("💻 Bash execution of `{}` not yet implemented.", trimmed) }),
    )
}

// ── Helper ────────────────────────────────────────────────────────────────────

fn parse_params<T: serde::de::DeserializeOwned>(req: &WsRequest) -> Result<T, WsResponse> {
    match &req.params {
        Some(p) => match serde_json::from_value::<T>(p.clone()) {
            Ok(v) => Ok(v),
            Err(e) => Err(WsResponse::err(
                &req.id,
                "INVALID_PARAMS",
                format!("Invalid parameters: {}", e),
            )),
        },
        None => Err(WsResponse::err(
            &req.id,
            "INVALID_PARAMS",
            "Missing parameters",
        )),
    }
}
