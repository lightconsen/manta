//! Slash Command System for Syscity Gateway
//!
//! Provides `/` commands via WebSocket RPC.
//! Commands are exposed via `commands.list` and executed via
//! `commands.execute`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::acp::{AcpSessionId, SpawnMode, SubagentConfig, ThreadBinding};
use crate::agent::TranscriptFormat;
use crate::gateway::command_provider::{CommandProviderHint, CommandProviderResolver};
use crate::gateway::protocol::*;
use crate::gateway::GatewayState;
use crate::tools::approval::{ApprovalDecision, ApprovalFilter};
use crate::tools::command_gate::UserLevel;
use crate::tools::mcp::{McpServerConfig, McpToolWrapper};

// ── Command Definitions
// ───────────────────────────────────────────────────────

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

/// Where a command is valid
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandScope {
    /// Works everywhere (default)
    #[default]
    Global,
    /// DM only
    DirectMessage,
    /// Channel only
    Channel,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub scope: CommandScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_hint: Option<CommandProviderHint>,
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
            aliases: Vec::new(),
            scope: CommandScope::Global,
            provider_hint: None,
        }
    }

    fn with_args(mut self, args: &str) -> Self {
        self.args = Some(args.to_string());
        self
    }

    fn with_aliases(mut self, aliases: &[&str]) -> Self {
        self.aliases = aliases.iter().map(|a| a.to_string()).collect();
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

    fn power(mut self) -> Self {
        self.tier = CommandTier::Power;
        self
    }
}

/// Built-in command catalog
pub fn built_in_commands() -> Vec<CommandDef> {
    vec![
        // Session
        CommandDef::new("new", "new", "Start a new session", CommandCategory::Session)
            .with_args("[model]")
            .with_aliases(&["clear"])
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
        // Model
        CommandDef::new(
            "model",
            "model",
            "Show or switch the active model",
            CommandCategory::Model,
        )
        .with_args("[name|#|status]"),
        CommandDef::new("think", "think", "Set thinking level", CommandCategory::Model)
            .with_args("<level>"),
        CommandDef::new("verbose", "verbose", "Toggle verbose output", CommandCategory::Model)
            .with_args("on|off|full"),
        CommandDef::new("fast", "fast", "Show or set fast mode", CommandCategory::Model)
            .with_args("[on|off|status]"),
        CommandDef::new("trace", "trace", "Toggle plugin trace", CommandCategory::Model)
            .with_args("on|off"),
        CommandDef::new(
            "reasoning",
            "reasoning",
            "Set reasoning visibility",
            CommandCategory::Model,
        )
        .with_args("[on|off|stream]"),
        CommandDef::new("queue", "queue", "Set queue behavior", CommandCategory::Model)
            .with_args("<mode>"),
        // Status / Query
        CommandDef::new("help", "help", "Show help summary", CommandCategory::Status)
            .with_aliases(&["commands"])
            .essential(),
        CommandDef::new("status", "status", "Show runtime status", CommandCategory::Status)
            .essential(),
        CommandDef::new("tools", "tools", "Show available tools", CommandCategory::Status)
            .with_args("[compact|verbose]"),
        CommandDef::new("whoami", "whoami", "Show your sender ID", CommandCategory::Status)
            .essential(),
        CommandDef::new("usage", "usage", "Show usage statistics", CommandCategory::Status)
            .with_args("[off|tokens|full|cost]"),
        CommandDef::new(
            "context",
            "context",
            "Show context assembly info",
            CommandCategory::Status,
        )
        .with_args("[list|detail|json]"),
        // Agents / ACP
        CommandDef::new("subagents", "subagents", "Manage sub-agents", CommandCategory::Agents)
            .with_args("list|kill|log|info|send|steer|spawn"),
        CommandDef::new("acp", "acp", "Manage ACP sessions", CommandCategory::Agents)
            .with_args("spawn|cancel|steer|close|sessions|status|..."),
        CommandDef::new("skill", "skill", "List or show skill details", CommandCategory::Agents)
            .with_args("[name]"),
        CommandDef::new("session", "session", "Manage session timeouts", CommandCategory::Session)
            .with_args("idle|max-age <duration|off>"),
        CommandDef::new("kill", "kill", "Abort sub-agent runs", CommandCategory::Agents)
            .with_args("<id|#|all>"),
        CommandDef::new("steer", "steer", "Send steering to a sub-agent", CommandCategory::Agents)
            .with_args("<id> <message>")
            .with_aliases(&["tell"]),
        CommandDef::new("focus", "focus", "Bind thread to session target", CommandCategory::Agents)
            .with_args("<target>"),
        CommandDef::new("unfocus", "unfocus", "Remove thread binding", CommandCategory::Agents),
        // Skills / Approval
        CommandDef::new(
            "allowlist",
            "allowlist",
            "Manage command allowlist",
            CommandCategory::Agents,
        )
        .with_args("[list|add|remove] ..."),
        CommandDef::new(
            "approve",
            "approve",
            "Resolve an approval prompt",
            CommandCategory::Agents,
        )
        .with_args("<id> <decision>")
        .with_aliases(&["y"]),
        CommandDef::new(
            "btw",
            "btw",
            "Side question without changing context",
            CommandCategory::Agents,
        )
        .with_args("<question>"),
        // Admin (owner-only)
        CommandDef::new("config", "config", "Read or write config", CommandCategory::Admin)
            .with_args("show|get|set|unset")
            .admin()
            .power(),
        CommandDef::new("plugins", "plugins", "Inspect or toggle plugins", CommandCategory::Admin)
            .with_args("list|install|enable|disable")
            .admin()
            .power(),
        CommandDef::new("mcp", "mcp", "Manage MCP server connections", CommandCategory::Admin)
            .with_args("show|connect|disconnect|tools|resources|prompts|call|read")
            .admin()
            .power(),
        CommandDef::new("debug", "debug", "Runtime debug overrides", CommandCategory::Admin)
            .with_args("show|set|unset|reset")
            .admin()
            .power(),
        CommandDef::new("restart", "restart", "Restart the gateway", CommandCategory::Admin)
            .admin()
            .power(),
        CommandDef::new("bash", "bash", "Run a host shell command", CommandCategory::Admin)
            .with_args("<command>")
            .admin()
            .power(),
        CommandDef::new("goal", "goal", "Execute a goal with auto-check conditions", CommandCategory::Agents)
            .with_args("<description> [--max-rounds N]"),
    ]
}

/// Parsed arguments for the `/help` command
#[derive(Debug, Default)]
struct HelpArgs {
    page: usize,
    tier: Option<CommandTier>,
}

impl HelpArgs {
    fn parse(args: &str) -> Self {
        let mut page = 1usize;
        let mut tier = None;

        // Handle `--tier <value>` (space-separated) and `--tier=<value>` patterns
        let tokens: Vec<&str> = args.split_whitespace().collect();
        let mut i = 0;
        while i < tokens.len() {
            let t = tokens[i];
            if let Some(val) = t.strip_prefix("--tier=") {
                tier = Self::parse_tier(val);
            } else if t == "--tier" {
                if let Some(val) = tokens.get(i + 1) {
                    tier = Self::parse_tier(val);
                    i += 1; // skip next token
                }
            } else if let Ok(n) = t.parse::<usize>() {
                page = n;
            }
            i += 1;
        }
        if page == 0 {
            page = 1;
        }
        Self { page, tier }
    }

    fn parse_tier(s: &str) -> Option<CommandTier> {
        match s.to_lowercase().as_str() {
            "essential" => Some(CommandTier::Essential),
            "standard" => Some(CommandTier::Standard),
            "power" => Some(CommandTier::Power),
            _ => None,
        }
    }
}

/// Structured help response payload
#[derive(Debug, Serialize)]
struct HelpPayload {
    text: String,
    page: usize,
    total_pages: usize,
    total_commands: usize,
    tier: Option<CommandTier>,
}

// ── handlers
// ──────────────────────────────────────────────────────────────────

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

    let normalized = params
        .command
        .to_lowercase()
        .trim_start_matches('/')
        .to_string();

    debug!("Executing command: /{} args='{}'", normalized, params.args);

    // Determine session_id for persistence
    let mut session_id = params.session_id.clone();
    if session_id.is_none() {
        session_id = conn.read().await.subscriptions.first().cloned();
    }

    // Persist user command input
    let user_text = if params.args.is_empty() {
        format!("/{}", normalized)
    } else {
        format!("/{}", params.command)
    };
    if let Some(ref sid) = session_id {
        if let Some(ref store) = state.agents.store {
            tracing::info!("Persisting command /{} to session {}", normalized, sid);
            if let Err(e) = store
                .append_message(&crate::agent::session_store::AppendMessageParams {
                    session_id: sid,
                    role: "user",
                    content: &user_text,
                    ..Default::default()
                })
                .await
            {
                tracing::warn!("Failed to save command input to session history: {}", e);
            }
        } else {
            tracing::warn!("No session_store available, cannot persist command /{}", normalized);
        }
    } else {
        tracing::warn!("No session_id for command /{}, cannot persist", normalized);
    }

    // Execute command and capture response
    let response = async {
        // Find the command definition
        let commands = built_in_commands();
        let def = match commands.iter().find(|c| {
            c.key == normalized || c.name == normalized || c.aliases.contains(&normalized)
        }) {
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

        // Local commands are only available in channel message contexts, not
        // via the WebSocket RPC path.
        if def.local {
            return WsResponse::err(
                &req.id,
                "LOCAL_COMMAND_NOT_AVAILABLE",
                format!("Command /{} is only available in channel conversations", def.key),
            );
        }

        // WebSocket RPC connections are treated as direct messages.
        let is_direct = true;
        match def.scope {
            CommandScope::Global => {}
            CommandScope::DirectMessage if is_direct => {}
            CommandScope::Channel => {
                return WsResponse::err(
                    &req.id,
                    "WRONG_SCOPE",
                    format!("Command /{} is only available in channels", def.key),
                );
            }
            _ => {
                return WsResponse::err(
                    &req.id,
                    "WRONG_SCOPE",
                    format!("Command /{} cannot be used here", def.key),
                );
            }
        }

        // Tier check against the user's configured level.
        let user_id = conn
            .read()
            .await
            .user_id
            .as_ref()
            .map(|u| u.0.clone())
            .unwrap_or_else(|| "anonymous".to_string());
        let user_level = state.auth.command_gate.user_level(&user_id);
        let required_level = match def.tier {
            CommandTier::Essential | CommandTier::Standard => UserLevel::User,
            CommandTier::Power => UserLevel::Admin,
        };
        if user_level < required_level {
            return WsResponse::err(
                &req.id,
                "INSUFFICIENT_TIER",
                format!(
                    "Command /{} requires {:?} access; you have {:?}",
                    def.key, required_level, user_level
                ),
            );
        }

        // Log any provider/model hint inferred for this command.
        if let Some(hint) = CommandProviderResolver::resolve(&def, user_level, None) {
            debug!(
                command = %def.key,
                provider = ?hint.provider,
                model = ?hint.model,
                reason = %hint.reason,
                "Provider hint resolved"
            );
        }

        // Dispatch by canonical key so aliases resolve to the same handler
        match def.key.as_str() {
            "help" | "commands" => handle_help(req, &params.args),
            "status" => handle_status(req, state).await,
            "whoami" => handle_whoami(req, conn).await,
            "stop" => handle_stop(req, conn, state).await,
            "reset" => handle_reset(req, conn, state).await,
            "model" => handle_model(req, conn, state, &params.args).await,
            "think" => handle_think(req, state, &params.args).await,
            "verbose" => handle_verbose(req, state, &params.args).await,
            "trace" => handle_trace(req, state, &params.args).await,
            "fast" => handle_fast(req, state, &params.args).await,
            "reasoning" => handle_reasoning(req, state, &params.args).await,
            "queue" => handle_queue(req, state, &params.args).await,
            "tools" => handle_tools(req, state, &params.args).await,
            "usage" => handle_usage(req, state, &params.args).await,
            "context" => handle_context(req, conn, state).await,
            "compact" => handle_compact(req, conn, state, &params.args).await,
            "session" => handle_session(req, conn, state, &params.args).await,
            "export-session" => handle_export_session(req, conn, state, &params.args).await,
            "subagents" => handle_subagents(req, conn, state, &params.args).await,
            "acp" => handle_acp(req, conn, state, &params.args).await,
            "steer" | "tell" => handle_steer(req, conn, state, &params.args).await,
            "kill" => handle_kill(req, conn, state, &params.args).await,
            "focus" => handle_focus(req, conn, state, &params.args).await,
            "unfocus" => handle_unfocus(req, conn, state).await,
            "skill" => handle_skill(req, state, &params.args).await,
            "allowlist" => handle_allowlist(req, state, &params.args).await,
            "approve" => handle_approve(req, state, &params.args).await,
            "btw" => handle_btw(req, state, &params.args).await,
            "config" => handle_config(req, state, &params.args).await,
            "plugins" => handle_plugins(req, state, &params.args).await,
            "mcp" => handle_mcp(req, state, &params.args).await,
            "debug" => handle_debug(req, state, &params.args).await,
            "restart" => handle_restart(req, state).await,
            "bash" => handle_bash(req, &params.args).await,
            "goal" => handle_goal(req, conn, state, &params.args).await,
            _ => WsResponse::err(
                &req.id,
                "NOT_HANDLED",
                format!("Command /{} is not handled server-side", def.key),
            ),
        }
    }
    .await;

    // Persist command result
    let result_text = if response.ok {
        response
            .payload
            .as_ref()
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        response
            .error
            .as_ref()
            .map(|e| format!("Command error: {}", e.message))
            .unwrap_or_else(|| "Command error".to_string())
    };

    if let Some(ref sid) = session_id {
        if let Some(ref store) = state.agents.store {
            if let Err(e) = store
                .append_message(&crate::agent::session_store::AppendMessageParams {
                    session_id: sid,
                    role: "assistant",
                    content: &result_text,
                    ..Default::default()
                })
                .await
            {
                tracing::warn!("Failed to save command result to session history: {}", e);
            }
        }
    }

    response
}

// ── Individual command handlers
// ───────────────────────────────────────────────

fn handle_help(req: &WsRequest, args: &str) -> WsResponse {
    let help_args = HelpArgs::parse(args);
    let all_commands = built_in_commands();

    // Apply tier filter
    let filtered: Vec<&CommandDef> = if let Some(tier) = help_args.tier {
        all_commands.iter().filter(|c| c.tier == tier).collect()
    } else {
        all_commands.iter().collect()
    };

    let total_commands = filtered.len();
    let page_size = 8usize;
    let total_pages = total_commands.div_ceil(page_size).max(1);
    let page = help_args.page.clamp(1, total_pages);
    let start = (page - 1) * page_size;
    let _end = start + page_size.min(total_commands.saturating_sub(start));
    let page_commands: Vec<&CommandDef> =
        filtered.into_iter().skip(start).take(page_size).collect();

    let mut lines = vec!["📋 **Syscity Commands**".to_string(), "".to_string()];

    let categories = [
        (CommandCategory::Session, "🗂️ Session"),
        (CommandCategory::Model, "🧠 Model"),
        (CommandCategory::Status, "ℹ️ Status"),
        (CommandCategory::Agents, "🤖 Agents"),
        (CommandCategory::Tools, "🛠️ Tools"),
        (CommandCategory::Admin, "🔒 Admin"),
    ];

    for (cat, title) in &categories {
        let cat_cmds: Vec<&&CommandDef> = page_commands
            .iter()
            .filter(|c| c.category == *cat)
            .collect();
        if cat_cmds.is_empty() {
            continue;
        }
        lines.push(format!("### {}", title));
        for c in cat_cmds {
            let admin_mark = if c.requires_admin { " `[admin]`" } else { "" };
            let args = c.args.as_deref().unwrap_or("");
            let args_display = if args.is_empty() {
                "".to_string()
            } else {
                format!(" `{}`", args)
            };
            let alias_display = if c.aliases.is_empty() {
                "".to_string()
            } else {
                let aliases: Vec<String> = c.aliases.iter().map(|a| format!("/{}", a)).collect();
                format!(" (alias: {})", aliases.join(", "))
            };
            lines.push(format!(
                "- `/{}{}`{}{}{}",
                c.name, args_display, alias_display, c.description, admin_mark
            ));
        }
        lines.push("".to_string());
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!(HelpPayload {
            text: lines.join("\n"),
            page,
            total_pages,
            total_commands,
            tier: help_args.tier,
        }),
    )
}

async fn handle_status(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agents = state.agents.agents.read().await.len();
    let sessions = state.agents.manager.read().await.len();

    let text = format!(
        "📊 **Status**\n\nActive agents: {}\nActive sessions: {}\nStatus: healthy",
        agents, sessions
    );

    WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
}

async fn handle_whoami(req: &WsRequest, conn: &Arc<RwLock<ProtocolConnection>>) -> WsResponse {
    let guard = conn.read().await;
    let user = guard
        .user_id
        .as_ref()
        .map(|u| u.to_string())
        .unwrap_or_else(|| "anonymous".to_string());
    let scopes = &guard.scopes;

    let text = format!("👤 **Whoami**\n\nUser: `{}`\nScopes: `{}`", user, scopes.join(", "));

    WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
}

async fn handle_stop(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();

    if let Some(sid) = session_id {
        if let Err(e) = state.agents.acp.cancel(sid.clone()).await {
            warn!("Failed to send stop signal for session {}: {}", sid, e);
        }
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("⏹️ Stop signal sent for session `{}`.", sid) }),
        );
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": "⏹️ No active session to stop." }))
}

async fn handle_reset(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();

    if let Some(sid) = session_id {
        {
            let mut mgr = state.agents.manager.write().await;
            mgr.terminate_session(&sid).await;
            mgr.create_session(sid.clone());
        }
        // Clear persisted history so the session truly resets
        if let Some(ref store) = state.agents.store {
            if let Err(e) = store.delete_session(&sid).await {
                tracing::warn!("Failed to delete session {} during reset: {}", sid, e);
            }
        }
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔄 Session `{}` reset.", sid) }),
        );
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": "🔄 No active session to reset." }))
}

async fn handle_model(
    req: &WsRequest,
    _conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "status" {
        let cfg = state.config.read().await;
        let settings = state.infra.runtime_settings.read().await;
        let override_model = settings.get("model.override").and_then(|v| v.as_str());
        let text = format!(
            "🧠 **Model Status**\n\nDefault: {} (provider: {})\nOverride: {}",
            cfg.model,
            cfg.model_provider,
            override_model.unwrap_or("none")
        );
        return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
    }

    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("model.override".to_string(), serde_json::json!(trimmed));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🧠 Model override set to '{}'.", trimmed) }),
    )
}

async fn handle_tools(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let verbose = args.trim() == "verbose";
    let tool_names = state.tools.registry.list();

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

async fn handle_usage(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode_arg = args.trim();
    let valid_modes = ["off", "tokens", "full", "cost"];

    if !mode_arg.is_empty() && valid_modes.contains(&mode_arg) {
        let mut settings = state.infra.runtime_settings.write().await;
        settings.insert("usage.mode".to_string(), serde_json::json!(mode_arg));
        return WsResponse::ok(
            &req.id,
            serde_json::json!({
                "text": format!("📊 Usage display mode set to '{}'.", mode_arg),
                "mode": mode_arg,
            }),
        );
    }

    if !mode_arg.is_empty() && !valid_modes.contains(&mode_arg) {
        return WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            format!("Usage: /usage [{}]", valid_modes.join("|")),
        );
    }

    let (mode, tokens, calls) = {
        let settings = state.infra.runtime_settings.read().await;
        let mode = settings
            .get("usage.mode")
            .and_then(|v| v.as_str())
            .unwrap_or("full")
            .to_string();
        let tokens = settings
            .get("usage.tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let calls = settings
            .get("usage.calls")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        (mode, tokens, calls)
    };

    let guard = state.agents.cost_guard.as_ref();
    let daily_cents = guard.daily_spend_cents();
    let daily_dollars = daily_cents as f64 / 100.0;
    let hourly_actions = guard.hourly_action_count();
    let daily_limit = guard.daily_limit_cents;
    let hourly_limit = guard.hourly_action_limit;
    let exceeded = guard.is_exceeded();

    let text = match mode.as_str() {
        "off" => "📊 Usage tracking display is disabled.".to_string(),
        "tokens" => {
            format!("📊 **Usage (tokens)**\n\nEstimated tokens: {}\nTool calls: {}", tokens, calls)
        }
        "cost" => format!(
            "📊 **Usage (cost)**\n\nDaily spend: ${:.2} ({} cents)\nHourly actions: {}",
            daily_dollars, daily_cents, hourly_actions
        ),
        _ => {
            let mut lines = vec!["📊 **Usage**".to_string()];
            lines.push(format!("Estimated tokens: {}", tokens));
            lines.push(format!("Tool calls: {}", calls));
            lines.push(format!("Daily spend: ${:.2} ({} cents)", daily_dollars, daily_cents));
            lines.push(format!("Hourly actions: {}", hourly_actions));
            if daily_limit > 0 {
                lines.push(format!("Daily limit: ${:.2}", daily_limit as f64 / 100.0));
            }
            if hourly_limit > 0 {
                lines.push(format!("Hourly action limit: {}", hourly_limit));
            }
            if exceeded {
                lines.push("⚠️ Budget limit exceeded.".to_string());
            }
            lines.join("\n")
        }
    };

    WsResponse::ok(&req.id, serde_json::json!({ "text": text, "mode": mode }))
}

async fn handle_compact(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();
    let instructions = args.trim();

    let Some(sid) = session_id else {
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "🗜️ No active session to compact." }),
        );
    };

    // Resolve agent for session
    let route = state.agents.router.resolve_by_session(&sid).await;
    let agents = state.agents.agents.read().await;
    let agent_handle = match agents.get(&route.agent_id) {
        Some(h) => h.clone(),
        None => {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🗜️ Agent not found for this session." }),
            );
        }
    };
    drop(agents);

    // Run context compaction via the Summarize strategy
    let compact_result = agent_handle.agent.compact_context(&sid).await;

    // Flush transcript to disk as a compaction step
    let export_result = state
        .infra
        .transcript_store
        .export(&sid, TranscriptFormat::Markdown)
        .await;

    let mut lines = vec![format!("🗜️ **Compacted session `{}`**", sid)];

    match compact_result {
        Some((before, after)) => {
            if after < before {
                lines.push(format!("Messages compressed: {} → {}", before, after));
            } else {
                lines.push(format!("Messages: {} (no reduction needed)", before));
            }
        }
        None => {
            lines.push("No context found to compact.".to_string());
        }
    }

    match export_result {
        Ok(path) => lines.push(format!("Transcript flushed to `{}`.", path.display())),
        Err(e) => lines.push(format!("Transcript export failed: {}", e)),
    }

    if !instructions.is_empty() {
        lines.push(format!("Instructions: {}", instructions));
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }))
}

async fn handle_bash(req: &WsRequest, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /bash <command>");
    }

    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(trimmed)
        .output()
        .await
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut lines = vec![format!("💻 `$ {}`", trimmed)];
            if !stdout.is_empty() {
                lines.push("\nstdout:".to_string());
                lines.push(stdout.to_string());
            }
            if !stderr.is_empty() {
                lines.push("\nstderr:".to_string());
                lines.push(stderr.to_string());
            }
            lines.push(format!("\nexit code: {}", output.status.code().unwrap_or(-1)));
            WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }))
        }
        Err(e) => WsResponse::err(&req.id, "EXEC_FAILED", format!("Failed to execute: {}", e)),
    }
}

async fn handle_goal(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();

    // Parse subcommands (e.g., /goal cancel <id>).
    let first_word = trimmed.split_whitespace().next().unwrap_or("");
    if first_word == "cancel" || first_word == "list" {
        let rest = trimmed.splitn(2, ' ').nth(1).unwrap_or("").trim();
        match first_word {
            "cancel" => {
                if rest.is_empty() {
                    return WsResponse::err(
                        &req.id,
                        "INVALID_ARGS",
                        "Usage: /goal cancel <goal_id>",
                    );
                }
                let cancelled = {
                    let mut cancellers = state.agents.goal_cancellers.write().await;
                    if let Some(token) = cancellers.remove(rest) {
                        token.cancel();
                        true
                    } else {
                        false
                    }
                };
                if cancelled {
                    return WsResponse::ok(
                        &req.id,
                        serde_json::json!({ "text": format!("🎯 Goal `{}` cancelled.", rest) }),
                    );
                } else {
                    return WsResponse::err(
                        &req.id,
                        "GOAL_NOT_FOUND",
                        format!("Goal `{}` not found or already completed.", rest),
                    );
                }
            }
            "list" => {
                let cancellers = state.agents.goal_cancellers.read().await;
                let ids: Vec<&String> = cancellers.keys().collect();
                if ids.is_empty() {
                    return WsResponse::ok(
                        &req.id,
                        serde_json::json!({ "text": "🎯 No active goals." }),
                    );
                }
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "text": format!("🎯 **Active Goals**\n\n{}", ids.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")),
                        "goals": ids,
                    }),
                );
            }
            _ => unreachable!(),
        }
    }

    let description = trimmed;
    if description.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /goal <description> [--max-rounds N]");
    }

    // Parse optional --max-rounds flag.
    let mut description = trimmed.to_string();
    let mut max_rounds: usize = 5;
    if let Some(pos) = trimmed.rfind("--max-rounds") {
        let before = &trimmed[..pos].trim();
        let rest = trimmed[pos..].trim();
        if let Some(val_str) = rest.split_whitespace().nth(1) {
            if let Ok(n) = val_str.parse::<usize>() {
                max_rounds = n.max(1);
                description = before.to_string();
            }
        }
    }

    if description.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Description required");
    }

    // Resolve the real session_id from the connection's subscriptions.
    let session_id = conn.read().await.subscriptions.first().cloned();

    // Parse the goal description into structured conditions using the LLM.
    let plan = match crate::goal::GoalPlan::parse_with_llm(
        &state.infra.model_router,
        &description,
        Some(max_rounds),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            return WsResponse::err(
                &req.id,
                "GOAL_PARSE_FAILED",
                format!("Failed to parse goal: {}", e),
            );
        }
    };

    let goal_id = format!("goal_{}", uuid::Uuid::new_v4().to_string());
    let sid = session_id.unwrap_or_else(|| "unknown".to_string());

    // Create event channel between GoalRunner and gateway.
    let (goal_tx, mut goal_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_tx = state.events.tx.clone();
    let gid = goal_id.clone();
    let s_for_relay = sid.clone();

    // Spawn event relay: GoalEvent → GatewayEvent.
    tokio::spawn(async move {
        while let Some(goal_event) = goal_rx.recv().await {
            let gw_event = crate::gateway::GatewayEvent::GoalProgress {
                goal_id: gid.clone(),
                session_id: s_for_relay.clone(),
                event: goal_event,
            };
            if let Err(e) = event_tx.send(gw_event) {
                warn!("[goal] Failed to broadcast event: {}", e);
                break;
            }
        }
    });

    // Create goal store for persistence (checkpoint after each round).
    let goal_store = crate::goal::persist::shared_store();

    // Create the cancel token and register it for /goal cancel.
    let runner = crate::goal::GoalRunner::new(
        &goal_id,
        &sid,
        plan,
        state.tools.registry.clone(),
        state.infra.model_router.clone(),
        goal_tx,
    )
    .with_store(goal_store.clone());
    let cancel_token = runner.cancel_token();
    {
        let mut cancellers = state.agents.goal_cancellers.write().await;
        cancellers.insert(goal_id.clone(), cancel_token);
    }

    // Spawn GoalRunner as background task — remove cancellers entry when done.
    let gid2 = goal_id.clone();
    let cancellers = state.agents.goal_cancellers.clone();
    tokio::spawn(async move {
        runner.run().await;
        // Clean up cancellers entry on completion.
        let mut c = cancellers.write().await;
        c.remove(&gid2);
    });

    WsResponse::ok(&req.id, serde_json::json!({
        "text": format!("🎯 Goal started: {}\nID: {}\nMax rounds: {}\n\nGoal events will appear in this session.", description, goal_id, max_rounds),
        "goal_id": goal_id,
    }))
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
        let handles = if let Some(ref sid) = session_id {
            state
                .agents
                .acp
                .list_session_subagents(&AcpSessionId(sid.clone()))
                .await
        } else {
            state.agents.acp.list_subagents().await
        };

        if handles.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🤖 No subagents found." }),
            );
        }

        let mut lines = vec![format!("🤖 **Subagents** ({} total)", handles.len())];
        for h in &handles {
            lines.push(format!(
                "- `{}` — status: `{:?}`, mode: `{:?}`, thread: `{}`",
                h.id, h.status, h.mode, h.thread_id
            ));
        }
        if let Some(sid) = session_id {
            lines.push(format!("Session: `{}`", sid));
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let sub = parts[0];
    let rest = parts.get(1).unwrap_or(&"").trim();

    match sub {
        "kill" => {
            if rest.is_empty() || rest == "all" {
                let session_id = conn.read().await.subscriptions.first().cloned();
                if let Some(sid) = session_id {
                    match state
                        .agents
                        .acp
                        .terminate_session(&AcpSessionId(sid.clone()))
                        .await
                    {
                        Ok(count) => WsResponse::ok(
                            &req.id,
                            serde_json::json!({
                                "text": format!(
                                    "💀 Terminated {} subagent(s) in session `{}`.",
                                    count, sid
                                )
                            }),
                        ),
                        Err(e) => WsResponse::err(
                            &req.id,
                            "KILL_FAILED",
                            format!("Failed to terminate session: {}", e),
                        ),
                    }
                } else {
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({ "text": "💀 No active session to kill." }),
                    )
                }
            } else {
                match state.agents.acp.kill_subagent(rest).await {
                    Ok(true) => WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!("💀 Subagent `{}` killed.", rest)
                        }),
                    ),
                    Ok(false) => WsResponse::err(
                        &req.id,
                        "AGENT_NOT_FOUND",
                        format!("Subagent `{}` not found.", rest),
                    ),
                    Err(e) => WsResponse::err(
                        &req.id,
                        "KILL_FAILED",
                        format!("Failed to kill `{}`: {}", rest, e),
                    ),
                }
            }
        }
        "log" => {
            let topics = state.agents.acp.bus_topics().await;
            if topics.is_empty() {
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": "📜 No ACP bus topics." }),
                );
            }
            let mut lines = vec!["📜 **ACP Bus Log**".to_string()];
            for topic in topics {
                let subscribers = state.agents.acp.bus_subscribers(&topic).await;
                lines.push(format!(
                    "- `{}` — {} subscriber(s): {}",
                    topic,
                    subscribers.len(),
                    subscribers.join(", ")
                ));
            }
            WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }))
        }
        "info" => {
            if rest.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /subagents info <id>");
            }
            let status = state.agents.acp.get_subagent_status(rest).await;
            let all = state.agents.acp.list_subagents().await;
            match all.iter().find(|h| h.id == rest) {
                Some(handle) => {
                    let text = format!(
                        "🤖 **Subagent `{}`**\n\nStatus: `{:?}`\nMode: `{:?}`\nThread: \
                         `{}`\nSession: `{}`\nParent: `{}`",
                        handle.id,
                        status.unwrap_or(handle.status),
                        handle.mode,
                        handle.thread_id,
                        handle.session_id,
                        handle.parent_id
                    );
                    WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
                }
                None => WsResponse::err(
                    &req.id,
                    "AGENT_NOT_FOUND",
                    format!("Subagent `{}` not found.", rest),
                ),
            }
        }
        "send" | "steer" => {
            if rest.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /subagents send|steer <id> <message>",
                );
            }
            let msg_parts: Vec<&str> = rest.splitn(2, ' ').collect();
            let target_id = msg_parts[0];
            let message = msg_parts.get(1).unwrap_or(&"").trim();
            if message.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /subagents send|steer <id> <message>",
                );
            }

            let guard = conn.read().await;
            let sender = guard
                .user_id
                .as_ref()
                .map(|u| u.0.clone())
                .unwrap_or_else(|| "user".to_string());
            let conversation_id = guard.subscriptions.first().cloned().unwrap_or_default();
            drop(guard);

            let incoming =
                crate::channels::IncomingMessage::new(sender, conversation_id, message.to_string());

            if sub == "send" {
                match state.agents.acp.send_message(target_id, incoming).await {
                    Ok(result) => WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!(
                                "🤖 Message sent to `{}`.\n\nResponse: {}",
                                target_id, result
                            )
                        }),
                    ),
                    Err(e) => WsResponse::err(
                        &req.id,
                        "SEND_FAILED",
                        format!("Failed to send to `{}`: {}", target_id, e),
                    ),
                }
            } else {
                match state
                    .agents
                    .acp
                    .steer_subagent(target_id, message.to_string())
                    .await
                {
                    Ok(result) => WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!(
                                "🤖 Steering sent to `{}`.\n\nResult: {}",
                                target_id, result
                            )
                        }),
                    ),
                    Err(e) => WsResponse::err(
                        &req.id,
                        "STEER_FAILED",
                        format!("Failed to steer `{}`: {}", target_id, e),
                    ),
                }
            }
        }
        "spawn" => {
            let session_id = conn.read().await.subscriptions.first().cloned();
            let Some(sid) = session_id else {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /subagents spawn requires an active session.",
                );
            };
            let route = state.agents.router.resolve_by_session(&sid).await;
            if route.agent_id.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "NO_PARENT_AGENT",
                    "No parent agent found for the active session.",
                );
            }

            let (_agent_type, system_prompt) = if rest.is_empty() {
                ("default".to_string(), None)
            } else {
                let mut words = rest.splitn(2, ' ');
                let at = words.next().unwrap_or("default").to_string();
                let prompt = words.next().map(|s| s.to_string());
                (at, prompt)
            };

            let config = SubagentConfig {
                system_prompt,
                mode: SpawnMode::Run,
                thread_binding: ThreadBinding::Auto,
                ..SubagentConfig::default()
            };

            match state
                .agents
                .acp
                .spawn_subagent(AcpSessionId(sid.clone()), route.agent_id, config)
                .await
            {
                Ok(handle) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "text": format!(
                            "🤖 Spawned subagent `{}` in session `{}`.",
                            handle.id, sid
                        )
                    }),
                ),
                Err(e) => WsResponse::err(
                    &req.id,
                    "SPAWN_FAILED",
                    format!("Failed to spawn subagent: {}", e),
                ),
            }
        }
        _ => WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /subagents list|kill|log|info|send|steer|spawn",
        ),
    }
}

async fn handle_skill(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        let mgr = state.tools.skills_manager.read().await;
        let skills = mgr.prefilter_skills("", 50, 0).await;
        let names: Vec<String> = skills.into_iter().map(|s| s.name).collect();
        let text = format!("🎯 **Skills** ({} total): {}", names.len(), names.join(", "));
        return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let name = parts[0];
    let _input = parts.get(1).unwrap_or(&"");

    let mgr = state.tools.skills_manager.read().await;
    match mgr.get_skill(name).await {
        Some(skill) => {
            let text = format!(
                "🎯 **Skill: {}**\n\nVersion: {}\nDescription: {}\nEnabled: {}\nEligible: {}",
                skill.name, skill.version, skill.description, skill.enabled, skill.is_eligible,
            );
            WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
        }
        None => WsResponse::err(&req.id, "SKILL_NOT_FOUND", format!("Skill '{}' not found.", name)),
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
            if let Ok(Some(status)) = state.agents.acp.get_status(sid.clone()).await {
                let text = format!(
                    "🤖 **ACP Session `{}`**\n\nState: `{:?}`\nMode: `{:?}`\nIteration: \
                     {}/{}\nQueue depth: {}",
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
        return WsResponse::ok(&req.id, serde_json::json!({ "text": "🤖 No active ACP session." }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let sub = parts[0];
    let rest = parts.get(1).unwrap_or(&"").trim();

    match sub {
        "spawn" => {
            let parent_id = if rest.is_empty() {
                let session_id = conn.read().await.subscriptions.first().cloned();
                if let Some(sid) = session_id {
                    let route = state.agents.router.resolve_by_session(&sid).await;
                    if route.agent_id.is_empty() {
                        None
                    } else {
                        Some(route.agent_id)
                    }
                } else {
                    None
                }
            } else {
                Some(rest.to_string())
            };
            let Some(parent_id) = parent_id else {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /acp spawn [parent_agent_id] (requires an active session or explicit \
                     parent)",
                );
            };
            let session_id = state.agents.acp.create_session(parent_id.clone()).await;
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "text": format!(
                        "🤖 Created ACP session `{}` for parent `{}`.",
                        session_id, parent_id
                    )
                }),
            )
        }
        "cancel" => {
            let sid = if rest.is_empty() {
                conn.read().await.subscriptions.first().cloned()
            } else {
                Some(rest.to_string())
            };
            if let Some(sid) = sid {
                if let Err(e) = state.agents.acp.cancel(sid.clone()).await {
                    warn!("Failed to cancel ACP session {}: {}", sid, e);
                }
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
                if let Err(e) = state
                    .agents
                    .acp
                    .terminate_session(&AcpSessionId(sid.clone()))
                    .await
                {
                    warn!("Failed to terminate ACP session {}: {}", sid, e);
                }
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🤖 ACP session `{}` terminated.", sid) }),
                );
            }
            WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /acp close [session_id]")
        }
        "steer" => {
            if rest.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /acp steer <id> <message>",
                );
            }
            let steer_parts: Vec<&str> = rest.splitn(2, ' ').collect();
            let target_id = steer_parts[0];
            let message = steer_parts.get(1).unwrap_or(&"").trim();
            if message.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /acp steer <id> <message>",
                );
            }
            match state
                .agents
                .acp
                .steer_subagent(target_id, message.to_string())
                .await
            {
                Ok(result) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "text": format!(
                            "🤖 Steering sent to `{}`.\n\nResult: {}",
                            target_id, result
                        )
                    }),
                ),
                Err(e) => WsResponse::err(
                    &req.id,
                    "STEER_FAILED",
                    format!("Failed to steer `{}`: {}", target_id, e),
                ),
            }
        }
        "sessions" => {
            let subagents = state.agents.acp.list_subagents().await;
            let mut session_ids: Vec<AcpSessionId> =
                subagents.iter().map(|h| h.session_id.clone()).collect();
            session_ids.sort_by(|a, b| a.0.cmp(&b.0));
            session_ids.dedup();

            if session_ids.is_empty() {
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": "🤖 No ACP sessions." }),
                );
            }

            let mut lines = vec![format!("🤖 **ACP Sessions** ({})", session_ids.len())];
            for sid in session_ids {
                match state.agents.acp.get_session_info(&sid).await {
                    Some(info) => lines.push(format!(
                        "- `{}` — parent `{}`, {} subagent(s), created {}",
                        info.id, info.parent_agent_id, info.subagent_count, info.created_at
                    )),
                    None => lines.push(format!("- `{}` — metadata unavailable", sid)),
                }
            }
            WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }))
        }
        "pause" | "resume" | "step" => {
            let sid = if rest.is_empty() {
                conn.read().await.subscriptions.first().cloned()
            } else {
                Some(rest.to_string())
            };
            let Some(sid) = sid else {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    format!("Usage: /acp {} [session_id]", sub),
                );
            };
            match sub {
                "pause" => {
                    if let Err(e) = state.agents.acp.pause(sid.clone()).await {
                        warn!("Failed to pause ACP session {}: {}", sid, e);
                    }
                }
                "resume" => {
                    if let Err(e) = state.agents.acp.resume(sid.clone()).await {
                        warn!("Failed to resume ACP session {}: {}", sid, e);
                    }
                }
                "step" => {
                    if let Err(e) = state.agents.acp.step(sid.clone()).await {
                        warn!("Failed to step ACP session {}: {}", sid, e);
                    }
                }
                _ => {
                    return WsResponse::err(
                        &req.id,
                        "INVALID_ARGS",
                        format!("Unknown ACP subcommand: {}", sub),
                    );
                }
            }
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "text": format!("🤖 Sent `{}` to session `{}`.", sub, sid)
                }),
            )
        }
        _ => WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /acp spawn|cancel|steer|close|sessions|status|pause|resume|step",
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
        conn.read()
            .await
            .user_id
            .as_ref()
            .map(|u| u.0.clone())
            .unwrap_or_else(|| "user".to_string()),
        conn.read()
            .await
            .subscriptions
            .first()
            .cloned()
            .unwrap_or_default(),
        message.to_string(),
    );

    match state.agents.acp.send_message(target_id, incoming).await {
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
            if let Err(e) = state.agents.acp.cancel(sid.clone()).await {
                warn!("Failed to send kill signal to session {}: {}", sid, e);
            }
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
    match state.agents.acp.shutdown_subagent(trimmed).await {
        Ok(true) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("💀 Subagent `{}` shutdown initiated.", trimmed) }),
        ),
        Ok(false) => WsResponse::err(
            &req.id,
            "AGENT_NOT_FOUND",
            format!("Subagent `{}` not found.", trimmed),
        ),
        Err(e) => {
            WsResponse::err(&req.id, "KILL_FAILED", format!("Failed to kill `{}`: {}", trimmed, e))
        }
    }
}

async fn handle_config(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let cfg = state.config.read().await;
        let settings = state.infra.runtime_settings.read().await;
        let mut lines = vec!["⚙️ **Config**".to_string()];
        lines.push(format!("Model: {} (provider: {})", cfg.model, cfg.model_provider));
        lines.push(format!("Host: {}:{}", cfg.host, cfg.port));
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
            let settings = state.infra.runtime_settings.read().await;
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
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /config set <key> <value>",
                );
            }
            let mut settings = state.infra.runtime_settings.write().await;
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
            let mut settings = state.infra.runtime_settings.write().await;
            settings.remove(key);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("⚙️ Removed key '{}'.", key) }),
            )
        }
        _ => WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /config [show|get|set|unset]"),
    }
}

async fn handle_plugins(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let plugins = state.infra.plugin_manager.list_plugins().await;
        if plugins.is_empty() {
            return WsResponse::ok(&req.id, serde_json::json!({ "text": "🔌 No plugins loaded." }));
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
            match state.infra.plugin_manager.set_enabled(rest, true).await {
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
            match state.infra.plugin_manager.set_enabled(rest, false).await {
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

// ── Model directive handlers ────────────────────────────────────────────────

async fn handle_think(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let level = args.trim();
    if level.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("think.level")
            .and_then(|v| v.as_str())
            .unwrap_or("medium");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🧠 Thinking level: {}", current) }),
        );
    }
    let valid = ["off", "minimal", "low", "medium", "high"];
    if !valid.contains(&level) {
        return WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            format!("Valid levels: {}", valid.join(", ")),
        );
    }
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("think.level".to_string(), serde_json::json!(level));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🧠 Thinking level set to '{}'.", level) }),
    )
}

async fn handle_verbose(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("verbose.mode")
            .and_then(|v| v.as_str())
            .unwrap_or("off");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔊 Verbose mode: {}", current) }),
        );
    }
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("verbose.mode".to_string(), serde_json::json!(mode));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🔊 Verbose mode set to '{}'.", mode) }),
    )
}

async fn handle_trace(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("trace.enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔍 Plugin trace: {}", if current { "on" } else { "off" }) }),
        );
    }
    let enabled = mode == "on";
    state.infra.plugin_manager.set_trace_enabled(enabled);
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("trace.enabled".to_string(), serde_json::json!(enabled));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🔍 Plugin trace {}.", if enabled { "enabled" } else { "disabled" }) }),
    )
}

async fn handle_fast(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() || mode == "status" {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("fast.mode")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let active_model = settings
            .get("fast.active_model")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({
                "text": format!(
                    "⚡ Fast mode: {} (active model: {})",
                    if current { "on" } else { "off" },
                    active_model
                )
            }),
        );
    }
    let enabled = mode == "on";

    if enabled {
        // Resolve the fast model alias and read the current default model once.
        let fast_model = state.infra.model_router.resolve_alias("fast").await;
        let current_model = state.config.read().await.model.clone();
        let active_model = fast_model.clone().unwrap_or_else(|| current_model.clone());

        {
            let mut settings = state.infra.runtime_settings.write().await;
            settings.insert("fast.original_model".to_string(), serde_json::json!(current_model));
            settings.insert("fast.active_model".to_string(), serde_json::json!(active_model));
            settings.insert("fast.mode".to_string(), serde_json::json!(true));
        }

        if let Some(fast_model) = fast_model {
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("⚡ Fast mode enabled. Model switched to '{}'.", fast_model) }),
            )
        } else {
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "⚡ Fast mode enabled (no fast alias configured, using current model)." }),
            )
        }
    } else {
        // Restore the original default model and clear fast state in a single
        // runtime_settings write. Config is no longer mutated directly.
        let (original, active_model) = {
            let settings = state.infra.runtime_settings.read().await;
            (
                settings
                    .get("fast.original_model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                settings
                    .get("fast.active_model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            )
        };

        {
            let mut settings = state.infra.runtime_settings.write().await;
            settings.insert("fast.mode".to_string(), serde_json::json!(false));
            if let Some(ref orig) = original {
                settings.insert("fast.active_model".to_string(), serde_json::json!(orig));
            } else {
                settings.remove("fast.active_model");
            }
        }

        WsResponse::ok(
            &req.id,
            serde_json::json!({
                "text": format!(
                    "⚡ Fast mode disabled. Model restored to '{}'.",
                    active_model.unwrap_or_else(|| "default".to_string())
                )
            }),
        )
    }
}

async fn handle_reasoning(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("reasoning.visibility")
            .and_then(|v| v.as_str())
            .unwrap_or("on");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("💭 Reasoning visibility: {}", current) }),
        );
    }
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("reasoning.visibility".to_string(), serde_json::json!(mode));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("💭 Reasoning visibility set to '{}'.", mode) }),
    )
}

async fn handle_queue(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("queue.mode")
            .and_then(|v| v.as_str())
            .unwrap_or("steer");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("📥 Queue mode: {}", current) }),
        );
    }
    let valid = ["steer", "interrupt", "followup"];
    if !valid.contains(&mode) {
        return WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            format!("Valid queue modes: {}", valid.join(", ")),
        );
    }
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("queue.mode".to_string(), serde_json::json!(mode));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("📥 Queue mode set to '{}'.", mode) }),
    )
}

// ── Context / Session handlers
// ────────────────────────────────────────────────

async fn handle_context(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();

    let Some(sid) = session_id else {
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "📜 No active session to inspect." }),
        );
    };

    // Resolve agent for session
    let route = state.agents.router.resolve_by_session(&sid).await;
    let agents = state.agents.agents.read().await;
    let Some(agent_handle) = agents.get(&route.agent_id) else {
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "📜 Agent not found for this session." }),
        );
    };

    match agent_handle.agent.context_info(&sid).await {
        Some((msg_count, token_count, max_tokens, sys_len, tool_iters)) => {
            let settings = state.infra.runtime_settings.read().await;
            let mut lines = vec![format!("📜 **Context for `{}`**", sid)];
            lines.push(format!("Messages: {}", msg_count));
            lines.push(format!("Tokens: {} / {}", token_count, max_tokens));
            lines.push(format!("System prompt length: {} chars", sys_len));
            lines.push(format!("Tool iterations: {}", tool_iters));
            if !settings.is_empty() {
                lines.push("\nRuntime settings:".to_string());
                for (k, v) in settings.iter() {
                    if k.starts_with("context.") || k.starts_with("session.") {
                        lines.push(format!("  {} = {}", k, v));
                    }
                }
            }
            WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }))
        }
        None => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("📜 No context found for session `{}`.", sid) }),
        ),
    }
}

fn parse_duration(s: &str) -> Option<std::time::Duration> {
    let s = s.trim();
    if s == "off" {
        return Some(std::time::Duration::from_secs(u64::MAX));
    }
    let num_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if num_end == 0 {
        return None;
    }
    let num: u64 = s[..num_end].parse().ok()?;
    let unit = &s[num_end..];
    match unit {
        "s" | "sec" | "secs" => Some(std::time::Duration::from_secs(num)),
        "m" | "min" | "mins" => Some(std::time::Duration::from_secs(num * 60)),
        "h" | "hr" | "hrs" => Some(std::time::Duration::from_secs(num * 3600)),
        "d" | "day" | "days" => Some(std::time::Duration::from_secs(num * 86400)),
        _ => None,
    }
}

async fn handle_session(
    req: &WsRequest,
    _conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /session idle|max-age <duration|off>",
        );
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let sub = parts[0];
    let rest = parts.get(1).unwrap_or(&"").trim();

    match sub {
        "idle" | "max-age" => {
            if rest.is_empty() {
                let settings = state.infra.runtime_settings.read().await;
                let key = format!("session.{}", sub);
                let current = settings
                    .get(&key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("⏱️ Session {}: {}", sub, current) }),
                );
            }
            match parse_duration(rest) {
                Some(duration) => {
                    {
                        let mut mgr = state.agents.manager.write().await;
                        mgr.set_timeout(duration);
                    }
                    let mut settings = state.infra.runtime_settings.write().await;
                    let key = format!("session.{}", sub);
                    settings.insert(key.clone(), serde_json::json!(rest));
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({ "text": format!("⏱️ Session {} set to '{}'.", sub, rest) }),
                    )
                }
                None => WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Invalid duration. Use: 30m, 1h, 2d, or off",
                ),
            }
        }
        _ => {
            WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /session idle|max-age <duration|off>")
        }
    }
}

async fn handle_export_session(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();
    let _path_hint = args.trim();

    if let Some(sid) = session_id {
        match state
            .infra
            .transcript_store
            .export(&sid, TranscriptFormat::Html)
            .await
        {
            Ok(path) => {
                let text =
                    format!("📄 **Session `{}` exported**\n\nHTML: `{}`", sid, path.display());
                return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
            }
            Err(e) => {
                return WsResponse::err(
                    &req.id,
                    "EXPORT_FAILED",
                    format!("Failed to export session: {}", e),
                );
            }
        }
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": "📄 No active session to export." }))
}

// ── Focus / Binding handlers
// ──────────────────────────────────────────────────

async fn handle_focus(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let target = args.trim();
    if target.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /focus <target>");
    }

    let session_id = conn.read().await.subscriptions.first().cloned();
    if let Some(sid) = session_id {
        let result = crate::inbound::router::RouteResult {
            agent_id: target.to_string(),
            workspace_id: None,
            persisted_binding: true,
            is_fallback: false,
        };
        state.agents.router.bind_session(&sid, &result).await;
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🎯 Session `{}` bound to agent '{}'.", sid, target) }),
        );
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": "🎯 No active session to focus." }))
}

async fn handle_unfocus(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();
    if let Some(sid) = session_id {
        state.agents.router.unbind_session(&sid).await;
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🎯 Session `{}` unbound.", sid) }),
        );
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": "🎯 No active session to unfocus." }))
}

// ── Allowlist / Approval handlers
// ─────────────────────────────────────────────

async fn handle_allowlist(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let levels = state.auth.command_gate.user_levels();
        if levels.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🛡️ No custom user levels configured." }),
            );
        }
        let mut lines = vec!["🛡️ **User Levels**".to_string()];
        for (user, level) in levels {
            lines.push(format!("- {}: {}", user, level));
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
    let sub = parts[0];

    match sub {
        "add" => {
            let user = parts.get(1).unwrap_or(&"").trim();
            let level_str = parts.get(2).unwrap_or(&"user").trim();
            if user.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /allowlist add <user> [chat|user|admin]",
                );
            }
            let level = match level_str {
                "chat" => UserLevel::Chat,
                "admin" => UserLevel::Admin,
                _ => UserLevel::User,
            };
            state.auth.command_gate.set_user_level(user, level);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("🛡️ Set {} to level '{}'.", user, level) }),
            )
        }
        "remove" => {
            let user = parts.get(1).unwrap_or(&"").trim();
            if user.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /allowlist remove <user>");
            }
            state.auth.command_gate.clear_user_level(user);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("🛡️ Cleared level for '{}'.", user) }),
            )
        }
        _ => WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /allowlist [list|add|remove]"),
    }
}

async fn handle_approve(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let pending = state
            .tools
            .approval_queue
            .list_pending(ApprovalFilter::default())
            .await;
        if pending.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "✅ No pending approvals." }),
            );
        }
        let mut lines = vec![format!("⏳ **Pending Approvals** ({})", pending.len())];
        for pa in &pending {
            lines.push(format!(
                "- {}: {} (risk: {:?}, by: {})",
                pa.id, pa.tool_name, pa.risk_level, pa.requested_by
            ));
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let id = parts[0];
    let decision_str = parts.get(1).unwrap_or(&"").trim();

    if id.is_empty() || decision_str.is_empty() {
        return WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /approve <id> approve|deny [reason]",
        );
    }

    let decision = match decision_str {
        "approve" | "yes" | "y" => ApprovalDecision::Approve,
        "deny" | "no" | "n" => ApprovalDecision::Deny {
            reason: "Denied via /approve command.".to_string(),
        },
        _ => {
            return WsResponse::err(
                &req.id,
                "INVALID_ARGS",
                "Decision must be 'approve' or 'deny'.",
            );
        }
    };

    if state.tools.approval_queue.resolve(id, decision).await {
        WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("✅ Approval '{}' resolved.", id) }),
        )
    } else {
        WsResponse::err(
            &req.id,
            "NOT_FOUND",
            format!("Approval '{}' not found or already resolved.", id),
        )
    }
}

async fn handle_btw(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let question = args.trim();
    if question.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /btw <question>");
    }

    let messages = vec![crate::providers::Message::user(question)];
    match state.infra.model_router.complete_auto(messages, None).await {
        Ok(response) => {
            let text = format!(
                "💡 **Side question** ({}):\n\n{}",
                response.model, response.message.content
            );
            WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
        }
        Err(e) => {
            WsResponse::err(&req.id, "COMPLETION_FAILED", format!("Failed to get response: {}", e))
        }
    }
}

// ── MCP / Debug / Restart handlers
// ────────────────────────────────────────────

async fn handle_mcp(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let servers = state.tools.mcp_manager.list_servers().await;
        if servers.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🔌 No MCP servers connected." }),
            );
        }
        let text = format!("🔌 **MCP Servers** ({}): {}", servers.len(), servers.join(", "));
        return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
    }

    let mut tokens = trimmed.split_whitespace();
    let sub = tokens.next().unwrap_or("");

    match sub {
        "connect" => {
            let rest: Vec<&str> = tokens.collect();
            if rest.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp connect <server_id> [command] [args...]",
                );
            }
            let server_id = rest[0].to_string();
            let config = if rest.len() > 1 {
                let base = state
                    .config
                    .read()
                    .await
                    .mcp
                    .servers
                    .get(&server_id)
                    .cloned()
                    .unwrap_or_default();
                let command = rest[1].to_string();
                let args = rest[2..].iter().map(|s| s.to_string()).collect();
                McpServerConfig {
                    command: Some(command),
                    args,
                    ..base
                }
            } else {
                match state
                    .config
                    .read()
                    .await
                    .mcp
                    .servers
                    .get(&server_id)
                    .cloned()
                {
                    Some(cfg) => cfg,
                    None => {
                        return WsResponse::err(
                            &req.id,
                            "INVALID_ARGS",
                            "Usage: /mcp connect <server_id> [command] [args...]",
                        );
                    }
                }
            };

            match state
                .tools
                .mcp_manager
                .connect(&server_id, config.clone())
                .await
            {
                Ok(tools) => {
                    if let Some(client_arc) = state.tools.mcp_manager.get_client(&server_id).await {
                        let max_tools = if config.max_tools == 0 {
                            tools.len()
                        } else {
                            config.max_tools.min(tools.len())
                        };
                        for tool in tools.iter().take(max_tools) {
                            let wrapper =
                                Arc::new(McpToolWrapper::new(client_arc.clone(), &server_id, tool));
                            state.tools.registry.register_dynamic(wrapper);
                        }
                    }
                    let text = format!(
                        "🔌 Connected MCP server '{}' ({} tool{} registered).",
                        server_id,
                        tools.len(),
                        if tools.len() == 1 { "" } else { "s" }
                    );
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": text,
                            "server_id": server_id,
                            "tools": tools.len(),
                        }),
                    )
                }
                Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
            }
        }
        "disconnect" => {
            let server_id = tokens.next().unwrap_or("");
            if server_id.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp disconnect <server_id>",
                );
            }
            match state.tools.mcp_manager.disconnect(server_id).await {
                Ok(()) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🔌 MCP server '{}' disconnected.", server_id) }),
                ),
                Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
            }
        }
        "tools" => {
            let server_id = tokens.next().unwrap_or("");
            if server_id.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /mcp tools <server_id>");
            }
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => {
                    let client = client_arc.read().await;
                    let tools: Vec<serde_json::Value> = client
                        .get_tools()
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "name": t.name,
                                "description": t.description,
                            })
                        })
                        .collect();
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!("🔌 {} tool(s) on '{}'", tools.len(), server_id),
                            "server_id": server_id,
                            "tools": tools,
                        }),
                    )
                }
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        "resources" => {
            let server_id = tokens.next().unwrap_or("");
            if server_id.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp resources <server_id>",
                );
            }
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => match client_arc.read().await.list_resources().await {
                    Ok(resources) => {
                        let items: Vec<serde_json::Value> = resources
                            .iter()
                            .map(|r| {
                                serde_json::json!({
                                    "uri": r.uri,
                                    "name": r.name,
                                    "description": r.description,
                                    "mime_type": r.mime_type,
                                })
                            })
                            .collect();
                        WsResponse::ok(
                            &req.id,
                            serde_json::json!({
                                "text": format!(
                                    "🔌 {} resource(s) on '{}'",
                                    items.len(),
                                    server_id
                                ),
                                "server_id": server_id,
                                "resources": items,
                            }),
                        )
                    }
                    Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
                },
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        "prompts" => {
            let server_id = tokens.next().unwrap_or("");
            if server_id.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /mcp prompts <server_id>");
            }
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => match client_arc.read().await.list_prompts().await {
                    Ok(prompts) => {
                        let items: Vec<serde_json::Value> = prompts
                            .iter()
                            .map(|p| {
                                serde_json::json!({
                                    "name": p.name,
                                    "description": p.description,
                                    "arguments": p.arguments,
                                })
                            })
                            .collect();
                        WsResponse::ok(
                            &req.id,
                            serde_json::json!({
                                "text": format!(
                                    "🔌 {} prompt(s) on '{}'",
                                    items.len(),
                                    server_id
                                ),
                                "server_id": server_id,
                                "prompts": items,
                            }),
                        )
                    }
                    Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
                },
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        "call" => {
            let server_id = tokens.next().unwrap_or("");
            let tool_name = tokens.next().unwrap_or("");
            let json_args = tokens.collect::<Vec<&str>>().join(" ");
            if server_id.is_empty() || tool_name.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp call <server_id> <tool_name> [json_args]",
                );
            }
            let params = if json_args.is_empty() {
                serde_json::json!({})
            } else {
                match serde_json::from_str(&json_args) {
                    Ok(v) => v,
                    Err(e) => {
                        return WsResponse::err(
                            &req.id,
                            "INVALID_ARGS",
                            format!("Invalid JSON arguments: {}", e),
                        );
                    }
                }
            };
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => {
                    match client_arc.read().await.call_tool(tool_name, params).await {
                        Ok(result) => WsResponse::ok(
                            &req.id,
                            serde_json::json!({
                                "text": format!("🔌 Tool '{}' returned result.", tool_name),
                                "server_id": server_id,
                                "tool": tool_name,
                                "result": result,
                            }),
                        ),
                        Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
                    }
                }
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        "read" => {
            let server_id = tokens.next().unwrap_or("");
            let uri = tokens.next().unwrap_or("");
            if server_id.is_empty() || uri.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp read <server_id> <uri>",
                );
            }
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => match client_arc.read().await.read_resource(uri).await {
                    Ok(contents) => WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!(
                                "🔌 Read {} content fragment(s) from '{}'.",
                                contents.len(),
                                uri
                            ),
                            "server_id": server_id,
                            "uri": uri,
                            "contents": contents,
                        }),
                    ),
                    Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
                },
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        _ => WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /mcp [show|connect|disconnect|tools|resources|prompts|call|read]",
        ),
    }
}

async fn handle_debug(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let settings = state.infra.runtime_settings.read().await;
        let mut lines = vec!["🐛 **Debug Overrides**".to_string()];
        if settings.is_empty() {
            lines.push("No runtime overrides set.".to_string());
        } else {
            for (k, v) in settings.iter() {
                lines.push(format!("  {} = {}", k, v));
            }
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
    let sub = parts[0];

    match sub {
        "set" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            let val = parts.get(2).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /debug set <key> <value>");
            }
            let mut settings = state.infra.runtime_settings.write().await;
            let json_val = serde_json::from_str(val).unwrap_or_else(|_| serde_json::json!(val));
            settings.insert(key.to_string(), json_val.clone());
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("🐛 Set {} = {}", key, json_val) }),
            )
        }
        "unset" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /debug unset <key>");
            }
            let mut settings = state.infra.runtime_settings.write().await;
            settings.remove(key);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("🐛 Removed key '{}'.", key) }),
            )
        }
        "reset" => {
            let mut settings = state.infra.runtime_settings.write().await;
            settings.clear();
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🐛 All runtime overrides cleared." }),
            )
        }
        _ => WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /debug [show|set|unset|reset]"),
    }
}

async fn handle_restart(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let state_for_restart = state.clone();
    let restart_handle = tokio::spawn(async move {
        // Give the response a moment to be sent, then perform graceful shutdown.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let shutdown_token = state_for_restart.shutdown_token.clone();
        shutdown_token.cancel();

        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            crate::gateway::lifecycle::stop_gateway(&shutdown_token, &state_for_restart),
        )
        .await
        {
            Ok(Ok(())) => info!("Graceful shutdown completed for restart"),
            Ok(Err(e)) => warn!("Graceful shutdown failed during restart: {}", e),
            Err(_) => warn!("Graceful shutdown timed out during restart"),
        }

        std::process::exit(0);
    });
    state
        .task_registry
        .insert_join("system:restart", restart_handle)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": "🔄 Gateway restart initiated. The process will exit gracefully." }),
    )
}

// ── Helper ────────────────────────────────────────────────────────────────────

#[allow(clippy::result_large_err)]
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
        None => Err(WsResponse::err(&req.id, "INVALID_PARAMS", "Missing parameters")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_args_default() {
        let args = HelpArgs::parse("");
        assert_eq!(args.page, 1);
        assert!(args.tier.is_none());
    }

    #[test]
    fn test_help_args_page_number() {
        let args = HelpArgs::parse("3");
        assert_eq!(args.page, 3);
    }

    #[test]
    fn test_help_args_page_zero_clamped() {
        let args = HelpArgs::parse("0");
        assert_eq!(args.page, 1);
    }

    #[test]
    fn test_help_args_tier_flag() {
        let args = HelpArgs::parse("--tier essential");
        assert_eq!(args.page, 1);
        assert_eq!(args.tier, Some(CommandTier::Essential));
    }

    #[test]
    fn test_help_args_tier_equals() {
        let args = HelpArgs::parse("--tier=power");
        assert_eq!(args.page, 1);
        assert_eq!(args.tier, Some(CommandTier::Power));
    }

    #[test]
    fn test_help_args_page_and_tier() {
        let args = HelpArgs::parse("2 --tier standard");
        assert_eq!(args.page, 2);
        assert_eq!(args.tier, Some(CommandTier::Standard));
    }

    #[test]
    fn test_help_args_invalid_tier_ignored() {
        let args = HelpArgs::parse("--tier invalid");
        assert!(args.tier.is_none());
        assert_eq!(args.page, 1);
    }

    #[test]
    fn test_help_pagination_page_count() {
        let cmds = built_in_commands();
        let total = cmds.len();
        let expected_pages = total.div_ceil(8).max(1);
        let page1_slice: Vec<&CommandDef> = cmds.iter().take(8).collect();
        assert_eq!(page1_slice.len(), 8.min(total));
        assert!(expected_pages >= 1);
    }

    #[test]
    fn test_tier_filter_essential() {
        let cmds = built_in_commands();
        let essential: Vec<&CommandDef> = cmds
            .iter()
            .filter(|c| c.tier == CommandTier::Essential)
            .collect();
        assert!(!essential.is_empty());
        assert!(essential.iter().any(|c| c.key == "help"));
        assert!(essential.iter().any(|c| c.key == "new"));
    }

    #[test]
    fn test_tier_filter_power() {
        let cmds = built_in_commands();
        let power: Vec<&CommandDef> = cmds
            .iter()
            .filter(|c| c.tier == CommandTier::Power)
            .collect();
        assert!(!power.is_empty());
        assert!(power.iter().any(|c| c.key == "config"));
        assert!(power.iter().any(|c| c.key == "bash"));
    }
}
