//! Slash Command System for Manta Gateway
//!
//! Provides OpenClaw-style `/` commands via WebSocket RPC.
//! Commands are exposed via `commands.list` and executed via `commands.execute`.

use crate::acp::AcpSessionId;
use crate::gateway::protocol::*;
use crate::gateway::GatewayState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

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
        "subagents" => handle_subagents(req, conn, state, &params.args).await,
        "acp" => handle_acp(req, conn, state, &params.args).await,
        "steer" => handle_steer(req, conn, state, &params.args).await,
        "kill" => handle_kill(req, conn, state, &params.args).await,
        "skill" => handle_skill(req, state, &params.args).await,
        "config" => handle_config(req, state, &params.args).await,
        "plugins" => handle_plugins(req, state, &params.args).await,
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
    state: &Arc<GatewayState>,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();

    if let Some(sid) = session_id {
        state.acp.cancel(sid.clone()).await;
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("⏹️ Stop signal sent for session `{}`.", sid) }),
        );
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": "⏹️ No active session to stop." }),
    )
}

async fn handle_reset(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();

    if let Some(sid) = session_id {
        {
            let mut mgr = state.session_manager.write().await;
            mgr.terminate_session(&sid);
            mgr.create_session(sid.clone());
        }
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔄 Session `{}` reset.", sid) }),
        );
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": "🔄 No active session to reset." }),
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

async fn handle_subagents(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let session_id = conn.read().await.subscriptions.first().cloned();
        if let Some(sid) = session_id {
            if let Some(status) = state.acp.get_status(sid.clone()).await {
                let text = format!(
                    "🤖 **Subagents for `{}`**\n\n\
                    Runtime state: `{:?}`\n\
                    Mode: `{:?}`\n\
                    Iteration: {}/{}\n\
                    Queue depth: {}",
                    sid,
                    status.runtime_state,
                    status.mode,
                    status.current_iteration,
                    status.max_iterations,
                    status.queue_depth,
                );
                return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
            }
        }
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "🤖 No active session." }),
        );
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🤖 Subagent command '{}' not yet implemented.", trimmed) }),
    )
}

async fn handle_skill(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        let mgr = state.skills_manager.read().await;
        let skills = mgr.prefilter_skills("", 50).await;
        let names: Vec<String> = skills.into_iter().map(|s| s.name).collect();
        let text = format!(
            "🎯 **Skills** ({} total): {}",
            names.len(),
            names.join(", ")
        );
        return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let name = parts[0];
    let _input = parts.get(1).unwrap_or(&"");

    let mgr = state.skills_manager.read().await;
    match mgr.get_skill(name).await {
        Some(skill) => {
            let text = format!(
                "🎯 **Skill: {}**\n\n\
                Version: {}\n\
                Description: {}\n\
                Enabled: {}\n\
                Eligible: {}",
                skill.name,
                skill.version,
                skill.description,
                skill.enabled,
                skill.is_eligible,
            );
            WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
        }
        None => WsResponse::err(
            &req.id,
            "SKILL_NOT_FOUND",
            format!("Skill '{}' not found.", name),
        ),
    }
}

async fn handle_acp(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "status" {
        let session_id = conn.read().await.subscriptions.first().cloned();
        if let Some(sid) = session_id {
            if let Some(status) = state.acp.get_status(sid.clone()).await {
                let text = format!(
                    "🤖 **ACP Session `{}`**\n\n\
                    State: `{:?}`\n\
                    Mode: `{:?}`\n\
                    Iteration: {}/{}\n\
                    Queue depth: {}",
                    sid,
                    status.runtime_state,
                    status.mode,
                    status.current_iteration,
                    status.max_iterations,
                    status.queue_depth,
                );
                return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
            }
        }
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "🤖 No active ACP session." }),
        );
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let sub = parts[0];
    let rest = parts.get(1).unwrap_or(&"").trim();

    match sub {
        "cancel" => {
            let sid = if rest.is_empty() {
                conn.read().await.subscriptions.first().cloned()
            } else {
                Some(rest.to_string())
            };
            if let Some(sid) = sid {
                state.acp.cancel(sid.clone()).await;
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🤖 ACP session `{}` cancelled.", sid) }),
                );
            }
            WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /acp cancel [session_id]")
        }
        "close" => {
            let sid = if rest.is_empty() {
                conn.read().await.subscriptions.first().cloned()
            } else {
                Some(rest.to_string())
            };
            if let Some(sid) = sid {
                let _ = state.acp.terminate_session(&AcpSessionId(sid.clone())).await;
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🤖 ACP session `{}` terminated.", sid) }),
                );
            }
            WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /acp close [session_id]")
        }
        _ => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🤖 ACP subcommand '{}' not yet implemented.", sub) }),
        ),
    }
}

async fn handle_steer(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /steer <id> <message>");
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let target_id = parts[0];
    let message = parts.get(1).unwrap_or(&"").trim();

    if message.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /steer <id> <message>");
    }

    let incoming = crate::channels::IncomingMessage::new(
        conn.read().await.user_id.as_ref().map(|u| u.0.clone()).unwrap_or_else(|| "user".to_string()),
        conn.read().await.subscriptions.first().cloned().unwrap_or_default(),
        message.to_string(),
    );

    match state.acp.send_message(target_id, incoming).await {
        Ok(result) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🤖 Steering sent to `{}`. Result: {}", target_id, result) }),
        ),
        Err(e) => WsResponse::err(
            &req.id,
            "STEER_FAILED",
            format!("Failed to steer `{}`: {}", target_id, e),
        ),
    }
}

async fn handle_kill(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "all" {
        let session_id = conn.read().await.subscriptions.first().cloned();
        if let Some(sid) = session_id {
            state.acp.cancel(sid.clone()).await;
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("💀 Kill signal sent to session `{}`.", sid) }),
            );
        }
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "💀 No active session to kill." }),
        );
    }

    // Try to shutdown the specific subagent
    match state.acp.shutdown_subagent(trimmed).await {
        Ok(true) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("💀 Subagent `{}` shutdown initiated.", trimmed) }),
        ),
        Ok(false) => WsResponse::err(
            &req.id,
            "AGENT_NOT_FOUND",
            format!("Subagent `{}` not found.", trimmed),
        ),
        Err(e) => WsResponse::err(
            &req.id,
            "KILL_FAILED",
            format!("Failed to kill `{}`: {}", trimmed, e),
        ),
    }
}

async fn handle_config(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let cfg = state.config.read().await;
        let settings = state.runtime_settings.read().await;
        let mut lines = vec!["⚙️ **Config**".to_string()];
        lines.push(format!("Model: {} (provider: {})", cfg.model, cfg.model_provider));
        lines.push(format!("Host: {}:{}", cfg.host, cfg.port));
        lines.push(format!("Tailscale: {}", if cfg.tailscale_enabled { "enabled" } else { "disabled" }));
        if !settings.is_empty() {
            lines.push("\nRuntime settings:".to_string());
            for (k, v) in settings.iter() {
                lines.push(format!("  {} = {}", k, v));
            }
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
    let sub = parts[0];

    match sub {
        "get" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /config get <key>");
            }
            let settings = state.runtime_settings.read().await;
            match settings.get(key) {
                Some(v) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("⚙️ {} = {}", key, v) }),
                ),
                None => WsResponse::err(&req.id, "NOT_FOUND", format!("Key '{}' not found.", key)),
            }
        }
        "set" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            let val = parts.get(2).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /config set <key> <value>");
            }
            let mut settings = state.runtime_settings.write().await;
            let json_val = serde_json::from_str(val).unwrap_or_else(|_| serde_json::json!(val));
            settings.insert(key.to_string(), json_val.clone());
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("⚙️ Set {} = {}", key, json_val) }),
            )
        }
        "unset" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /config unset <key>");
            }
            let mut settings = state.runtime_settings.write().await;
            settings.remove(key);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("⚙️ Removed key '{}'.", key) }),
            )
        }
        _ => WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /config [show|get|set|unset]",
        ),
    }
}

async fn handle_plugins(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let plugins = state.plugin_manager.list_plugins().await;
        if plugins.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🔌 No plugins loaded." }),
            );
        }
        let mut lines = vec![format!("🔌 **Plugins** ({} total)", plugins.len())];
        for p in &plugins {
            lines.push(format!(
                "- {} ({}) — {} [{}]",
                p.name(),
                p.id(),
                p.manifest.description,
                if p.enabled { "enabled" } else { "disabled" }
            ));
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let sub = parts[0];
    let rest = parts.get(1).unwrap_or(&"").trim();

    match sub {
        "enable" => {
            if rest.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /plugins enable <id>");
            }
            match state.plugin_manager.set_enabled(rest, true).await {
                Ok(()) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🔌 Plugin '{}' enabled.", rest) }),
                ),
                Err(e) => WsResponse::err(&req.id, "PLUGIN_ERROR", format!("{}", e)),
            }
        }
        "disable" => {
            if rest.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /plugins disable <id>");
            }
            match state.plugin_manager.set_enabled(rest, false).await {
                Ok(()) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🔌 Plugin '{}' disabled.", rest) }),
                ),
                Err(e) => WsResponse::err(&req.id, "PLUGIN_ERROR", format!("{}", e)),
            }
        }
        _ => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔌 Plugin command '{}' not yet implemented.", sub) }),
        ),
    }
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
