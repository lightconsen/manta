//! Slash Command System for Syscity Gateway
//!
//! Provides OpenClaw-style `/` commands via WebSocket RPC.
//! Commands are exposed via `commands.list` and executed via `commands.execute`.

use crate::acp::AcpSessionId;
use crate::agent::TranscriptFormat;
use crate::gateway::protocol::*;
use crate::gateway::GatewayState;
use crate::tools::approval::{ApprovalDecision, ApprovalFilter};
use crate::tools::command_gate::UserLevel;
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
        CommandDef::new("clear", "clear", "Clear chat history", CommandCategory::Session).local(),
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
        CommandDef::new("help", "help", "Show help summary", CommandCategory::Status).essential(),
        CommandDef::new(
            "commands",
            "commands",
            "Show full command catalog",
            CommandCategory::Status,
        )
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
            .with_args("<id> <message>"),
        CommandDef::new("focus", "focus", "Bind thread to session target", CommandCategory::Agents)
            .with_args("<target>"),
        CommandDef::new("unfocus", "unfocus", "Remove thread binding", CommandCategory::Agents),
        CommandDef::new("tell", "tell", "Alias for steer", CommandCategory::Agents)
            .with_args("<id> <message>"),
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
        .with_args("<id> <decision>"),
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
            .admin(),
        CommandDef::new("plugins", "plugins", "Inspect or toggle plugins", CommandCategory::Admin)
            .with_args("list|install|enable|disable")
            .admin(),
        CommandDef::new("mcp", "mcp", "Manage MCP server connections", CommandCategory::Admin)
            .with_args("show|get|set|unset")
            .admin(),
        CommandDef::new("debug", "debug", "Runtime debug overrides", CommandCategory::Admin)
            .with_args("show|set|unset|reset")
            .admin(),
        CommandDef::new("restart", "restart", "Restart the gateway", CommandCategory::Admin)
            .admin(),
        CommandDef::new("bash", "bash", "Run a host shell command", CommandCategory::Admin)
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
        if let Some(ref store) = state.session_store {
            tracing::info!("Persisting command /{} to session {}", normalized, sid);
            if let Err(e) = store
                .append_message(&crate::agent::session_store::AppendMessageParams {
                    session_id: sid, role: "user", content: &user_text, ..Default::default()
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
        let def = match commands
            .iter()
            .find(|c| c.key == normalized || c.name == normalized)
        {
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
            "think" => handle_think(req, state, &params.args).await,
            "verbose" => handle_verbose(req, state, &params.args).await,
            "trace" => handle_trace(req, state, &params.args).await,
            "fast" => handle_fast(req, state, &params.args).await,
            "reasoning" => handle_reasoning(req, state, &params.args).await,
            "queue" => handle_queue(req, state, &params.args).await,
            "tools" => handle_tools(req, state, &params.args).await,
            "usage" => handle_usage(req, state).await,
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
            "restart" => handle_restart(req).await,
            "bash" => handle_bash(req, &params.args).await,
            _ => WsResponse::err(
                &req.id,
                "NOT_IMPLEMENTED",
                format!("Command /{} is not yet implemented", normalized),
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
        if let Some(ref store) = state.session_store {
            if let Err(e) = store
                .append_message(&crate::agent::session_store::AppendMessageParams {
                    session_id: sid, role: "assistant", content: &result_text, ..Default::default()
                })
                .await
            {
                tracing::warn!("Failed to save command result to session history: {}", e);
            }
        }
    }

    response
}

// ── Individual command handlers ───────────────────────────────────────────────

fn handle_help(req: &WsRequest) -> WsResponse {
    let commands = built_in_commands();
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
        let cat_cmds: Vec<&CommandDef> = commands.iter().filter(|c| c.category == *cat).collect();
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
            lines
                .push(format!("- `/{}{}` — {}{}", c.name, args_display, c.description, admin_mark));
        }
        lines.push("".to_string());
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
        state.acp.cancel(sid.clone()).await;
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
            let mut mgr = state.session_manager.write().await;
            mgr.terminate_session(&sid);
            mgr.create_session(sid.clone());
        }
        // Clear persisted history so the session truly resets
        if let Some(ref store) = state.session_store {
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
        let settings = state.runtime_settings.read().await;
        let override_model = settings.get("model.override").and_then(|v| v.as_str());
        let text = format!(
            "🧠 **Model Status**\n\nDefault: {} (provider: {})\nOverride: {}",
            cfg.model,
            cfg.model_provider,
            override_model.unwrap_or("none")
        );
        return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
    }

    let mut settings = state.runtime_settings.write().await;
    settings.insert("model.override".to_string(), serde_json::json!(trimmed));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🧠 Model override set to '{}'.", trimmed) }),
    )
}

async fn handle_tools(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
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

async fn handle_usage(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let settings = state.runtime_settings.read().await;
    let tokens = settings
        .get("usage.tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let calls = settings
        .get("usage.calls")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let text = format!(
        "📊 **Usage**\n\nEstimated tokens: {}\nTool calls: {}\n\nFull cost tracking not yet implemented.",
        tokens, calls
    );
    WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
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
    let route = state.agent_router.resolve_by_session(&sid).await;
    let agents = state.agents.read().await;
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
        return WsResponse::ok(&req.id, serde_json::json!({ "text": "🤖 No active session." }));
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🤖 Subagent command '{}' not yet implemented.", trimmed) }),
    )
}

async fn handle_skill(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        let mgr = state.skills_manager.read().await;
        let skills = mgr.prefilter_skills("", 50).await;
        let names: Vec<String> = skills.into_iter().map(|s| s.name).collect();
        let text = format!("🎯 **Skills** ({} total): {}", names.len(), names.join(", "));
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
        return WsResponse::ok(&req.id, serde_json::json!({ "text": "🤖 No active ACP session." }));
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
                let _ = state
                    .acp
                    .terminate_session(&AcpSessionId(sid.clone()))
                    .await;
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
        Err(e) => {
            WsResponse::err(&req.id, "KILL_FAILED", format!("Failed to kill `{}`: {}", trimmed, e))
        }
    }
}

async fn handle_config(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let cfg = state.config.read().await;
        let settings = state.runtime_settings.read().await;
        let mut lines = vec!["⚙️ **Config**".to_string()];
        lines.push(format!("Model: {} (provider: {})", cfg.model, cfg.model_provider));
        lines.push(format!("Host: {}:{}", cfg.host, cfg.port));
        lines.push(format!(
            "Tailscale: {}",
            if cfg.tailscale_enabled {
                "enabled"
            } else {
                "disabled"
            }
        ));
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
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /config set <key> <value>",
                );
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
        _ => WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /config [show|get|set|unset]"),
    }
}

async fn handle_plugins(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let plugins = state.plugin_manager.list_plugins().await;
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

// ── Model directive handlers ────────────────────────────────────────────────

async fn handle_think(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let level = args.trim();
    if level.is_empty() {
        let settings = state.runtime_settings.read().await;
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
    let mut settings = state.runtime_settings.write().await;
    settings.insert("think.level".to_string(), serde_json::json!(level));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🧠 Thinking level set to '{}'.", level) }),
    )
}

async fn handle_verbose(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.runtime_settings.read().await;
        let current = settings
            .get("verbose.mode")
            .and_then(|v| v.as_str())
            .unwrap_or("off");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔊 Verbose mode: {}", current) }),
        );
    }
    let mut settings = state.runtime_settings.write().await;
    settings.insert("verbose.mode".to_string(), serde_json::json!(mode));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🔊 Verbose mode set to '{}'.", mode) }),
    )
}

async fn handle_trace(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.runtime_settings.read().await;
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
    state.plugin_manager.set_trace_enabled(enabled);
    let mut settings = state.runtime_settings.write().await;
    settings.insert("trace.enabled".to_string(), serde_json::json!(enabled));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🔍 Plugin trace {}.", if enabled { "enabled" } else { "disabled" }) }),
    )
}

async fn handle_fast(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() || mode == "status" {
        let settings = state.runtime_settings.read().await;
        let current = settings
            .get("fast.mode")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("⚡ Fast mode: {}", if current { "on" } else { "off" }) }),
        );
    }
    let enabled = mode == "on";
    let mut settings = state.runtime_settings.write().await;

    if enabled {
        // Save current model and switch to fast alias
        let current_model = state.config.read().await.model.clone();
        settings.insert("fast.original_model".to_string(), serde_json::json!(current_model));
        if let Some(fast_model) = state.model_router.resolve_alias("fast").await {
            state.config.write().await.model = fast_model.clone();
            settings.insert("fast.mode".to_string(), serde_json::json!(true));
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("⚡ Fast mode enabled. Model switched to '{}'.", fast_model) }),
            );
        }
        settings.insert("fast.mode".to_string(), serde_json::json!(true));
        WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "⚡ Fast mode enabled (no fast alias configured, using current model)." }),
        )
    } else {
        // Restore original model
        let original = settings.get("fast.original_model").and_then(|v| v.as_str());
        if let Some(orig) = original {
            state.config.write().await.model = orig.to_string();
        }
        settings.insert("fast.mode".to_string(), serde_json::json!(false));
        WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "⚡ Fast mode disabled. Model restored." }),
        )
    }
}

async fn handle_reasoning(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.runtime_settings.read().await;
        let current = settings
            .get("reasoning.visibility")
            .and_then(|v| v.as_str())
            .unwrap_or("on");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("💭 Reasoning visibility: {}", current) }),
        );
    }
    let mut settings = state.runtime_settings.write().await;
    settings.insert("reasoning.visibility".to_string(), serde_json::json!(mode));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("💭 Reasoning visibility set to '{}'.", mode) }),
    )
}

async fn handle_queue(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.runtime_settings.read().await;
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
    let mut settings = state.runtime_settings.write().await;
    settings.insert("queue.mode".to_string(), serde_json::json!(mode));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("📥 Queue mode set to '{}'.", mode) }),
    )
}

// ── Context / Session handlers ────────────────────────────────────────────────

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
    let route = state.agent_router.resolve_by_session(&sid).await;
    let agents = state.agents.read().await;
    let Some(agent_handle) = agents.get(&route.agent_id) else {
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "📜 Agent not found for this session." }),
        );
    };

    match agent_handle.agent.context_info(&sid).await {
        Some((msg_count, token_count, max_tokens, sys_len, tool_iters)) => {
            let settings = state.runtime_settings.read().await;
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
                let settings = state.runtime_settings.read().await;
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
                    let mut mgr = state.session_manager.write().await;
                    mgr.set_timeout(duration);
                    let mut settings = state.runtime_settings.write().await;
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

// ── Focus / Binding handlers ──────────────────────────────────────────────────

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
            created_binding: true,
        };
        state.agent_router.bind_session(&sid, &result).await;
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
        state.agent_router.unbind_session(&sid).await;
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🎯 Session `{}` unbound.", sid) }),
        );
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": "🎯 No active session to unfocus." }))
}

// ── Allowlist / Approval handlers ─────────────────────────────────────────────

async fn handle_allowlist(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let levels = state.command_gate.user_levels();
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
            state.command_gate.set_user_level(user, level);
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
            state.command_gate.clear_user_level(user);
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

    if state.approval_queue.resolve(id, decision).await {
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
    match state.model_router.complete_auto(messages, None).await {
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

// ── MCP / Debug / Restart handlers ────────────────────────────────────────────

async fn handle_mcp(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let servers = state.mcp_manager.list_servers().await;
        if servers.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🔌 No MCP servers connected." }),
            );
        }
        let text = format!("🔌 **MCP Servers** ({}): {}", servers.len(), servers.join(", "));
        return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let sub = parts[0];
    let rest = parts.get(1).unwrap_or(&"").trim();

    match sub {
        "disconnect" => {
            if rest.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp disconnect <server_id>",
                );
            }
            match state.mcp_manager.disconnect(rest).await {
                Ok(()) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🔌 MCP server '{}' disconnected.", rest) }),
                ),
                Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
            }
        }
        _ => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔌 MCP subcommand '{}' not yet implemented.", sub) }),
        ),
    }
}

async fn handle_debug(req: &WsRequest, state: &Arc<GatewayState>, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let settings = state.runtime_settings.read().await;
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
            let mut settings = state.runtime_settings.write().await;
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
            let mut settings = state.runtime_settings.write().await;
            settings.remove(key);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("🐛 Removed key '{}'.", key) }),
            )
        }
        "reset" => {
            let mut settings = state.runtime_settings.write().await;
            settings.clear();
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🐛 All runtime overrides cleared." }),
            )
        }
        _ => WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /debug [show|set|unset|reset]"),
    }
}

async fn handle_restart(req: &WsRequest) -> WsResponse {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        std::process::exit(0);
    });
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": "🔄 Gateway restart initiated. The process will exit in 1 second." }),
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
