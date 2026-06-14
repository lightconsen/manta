//! Slash command parser and executor.

use crate::tui::actions::TuiAction;
use crate::tui::error::TuiError;
use crate::tui::event_loop::{fetch_commands, fetch_config};
use crate::tui::state::{AppState, InputMode, MessageStatus, Popup};
use crate::tui::ws_client::WsClient;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Commands handled locally by the TUI.
const LOCAL_COMMANDS: &[&str] = &[
    "new", "clear", "quit", "exit", "help", "config", "sessions", "status", "tools", "model",
];

/// Inline shortcuts that may be embedded in normal chat messages.
const INLINE_SHORTCUTS: &[&str] = &["help", "commands", "status", "whoami"];

/// Parse a slash command line into `(name, args)`.
pub fn parse_slash_command(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix('/') {
        let mut parts = rest.splitn(2, ' ');
        let name = parts.next()?;
        let args = parts.next().unwrap_or("").trim();
        Some((name, args))
    } else {
        None
    }
}

/// Check whether `name` is a locally-handled TUI command.
pub fn is_local_command(name: &str) -> bool {
    LOCAL_COMMANDS.contains(&name)
}

/// Execute a slash command.
pub async fn handle_slash_command(
    line: &str,
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
) -> Result<(), TuiError> {
    let (name, args) = match parse_slash_command(line) {
        Some(p) => p,
        None => return Ok(()),
    };

    if is_local_command(name) {
        handle_local_command(name, args, state, ws_client).await
    } else {
        execute_remote_command(name, args, state, ws_client).await
    }
}

/// Handle one of the built-in local TUI commands.
async fn handle_local_command(
    name: &str,
    args: &str,
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
) -> Result<(), TuiError> {
    match name {
        "new" => create_session(state, ws_client).await,
        "clear" => {
            let mut s = state.write().await;
            s.messages.clear();
            s.scroll_offset = 0;
            Ok(())
        }
        "status" => {
            match ws_client.request("system.presence", None).await {
                Ok(value) => {
                    let mut s = state.write().await;
                    s.toast(format!("Status: {}", value));
                }
                Err(e) => {
                    let mut s = state.write().await;
                    s.error_toast(format!("Status failed: {}", e));
                }
            }
            Ok(())
        }
        "tools" => {
            match ws_client.request("commands.list", None).await {
                Ok(value) => {
                    let mut s = state.write().await;
                    let count = value.as_array().map(|a| a.len()).unwrap_or(0);
                    s.toast(format!("{} commands available", count));
                }
                Err(e) => {
                    let mut s = state.write().await;
                    s.error_toast(format!("Tools failed: {}", e));
                }
            }
            Ok(())
        }
        "model" => {
            if args.is_empty() {
                let mut s = state.write().await;
                s.error_toast("Usage: /model <model-id>");
                return Ok(());
            }
            match ws_client
                .request("models.set_default", Some(serde_json::json!({ "model": args })))
                .await
            {
                Ok(_) => {
                    let mut s = state.write().await;
                    s.toast(format!("Default model set to {}", args));
                }
                Err(e) => {
                    let mut s = state.write().await;
                    s.error_toast(format!("Model switch failed: {}", e));
                }
            }
            Ok(())
        }
        "help" => {
            {
                let mut s = state.write().await;
                s.popup = Popup::Help;
                s.input_mode = InputMode::Popup;
            }
            fetch_commands(ws_client, Arc::clone(&state)).await
        }
        "config" => {
            {
                let mut s = state.write().await;
                s.popup = Popup::ConfigEditor;
                s.input_mode = InputMode::Popup;
            }
            fetch_config(ws_client, Arc::clone(&state)).await
        }
        "quit" | "exit" => {
            let mut s = state.write().await;
            s.should_quit = true;
            Ok(())
        }
        "sessions" => {
            match ws_client.request("sessions.list", None).await {
                Ok(value) => {
                    let mut s = state.write().await;
                    if let Some(items) = value.as_array() {
                        s.sessions = items
                            .iter()
                            .filter_map(|v| {
                                Some(crate::tui::state::SessionSummary {
                                    id: v.get("id")?.as_str()?.to_string(),
                                    label: v
                                        .get("name")
                                        .or_else(|| v.get("label"))
                                        .and_then(|v| v.as_str())
                                        .map(String::from),
                                    agent_id: v
                                        .get("agent_id")
                                        .and_then(|v| v.as_str())
                                        .map(String::from),
                                    selected: false,
                                })
                            })
                            .collect();
                        if let Some(current) = s.current_session.clone() {
                            s.switch_session(&current);
                        }
                    }
                    let count = s.sessions.len();
                    s.toast(format!("{} sessions listed", count));
                }
                Err(e) => {
                    let mut s = state.write().await;
                    s.error_toast(format!("Sessions failed: {}", e));
                }
            }
            Ok(())
        }
        _ => {
            let mut s = state.write().await;
            s.error_toast(format!("Unknown command: /{}", name));
            Ok(())
        }
    }
}

/// Forward a non-local slash command to the gateway via `commands.execute`.
async fn execute_remote_command(
    name: &str,
    args: &str,
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
) -> Result<(), TuiError> {
    let (sid, assistant_id) = {
        let mut s = state.write().await;
        let sid = s
            .current_session
            .clone()
            .unwrap_or_else(|| format!("tui:{}", uuid::Uuid::new_v4()));
        s.current_session = Some(sid.clone());
        s.ensure_session(&sid);
        let assistant_id = format!("assistant_{}", s.messages.len());
        s.append_complete_assistant_message(&assistant_id, "Running command...");
        (sid, assistant_id)
    };

    let params = serde_json::json!({
        "command": name,
        "args": args,
        "session_id": sid,
    });

    let result = ws_client.request("commands.execute", Some(params)).await;

    let mut s = state.write().await;
    if let Some(msg) = s.messages.iter_mut().find(|m| m.id == assistant_id) {
        match result {
            Ok(payload) => {
                let text = payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| serde_json::to_string(&payload).unwrap_or_default());
                msg.content = text;
                msg.status = MessageStatus::Complete;
            }
            Err(e) => {
                msg.content = format!("Command error: {}", e);
                msg.status = MessageStatus::Error(format!("{}", e));
            }
        }
    }
    Ok(())
}

/// Create a new session and select it.
async fn create_session(
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
) -> Result<(), TuiError> {
    let result = ws_client.request("sessions.create", None).await?;
    if let Some(sid) = result.get("session_id").and_then(|v| v.as_str()) {
        {
            let mut s = state.write().await;
            s.ensure_session(sid);
            s.switch_session(sid);
        }
        ws_client
            .request("sessions.subscribe", Some(serde_json::json!({ "session_id": sid })))
            .await?;
        let mut s = state.write().await;
        s.toast(format!("Created session {}", sid));
    }
    Ok(())
}

/// Convert a `TuiAction::SendMessage` into a slash command when the input
/// starts with `/`.
pub fn action_for_input(action: TuiAction, buffer: &str) -> TuiAction {
    match action {
        TuiAction::SendMessage if buffer.trim_start().starts_with('/') => {
            TuiAction::RunSlashCommand(buffer.trim().to_string())
        }
        other => other,
    }
}

/// Update the command palette filter based on the current input buffer.
pub fn update_palette(state: &mut AppState) {
    let query = state.input_buffer.trim_start_matches('/');
    let all = state.command_list.clone();
    let filtered: Vec<_> = all
        .into_iter()
        .filter(|c| c.matches(query))
        .collect();
    state.palette_commands = filtered;
    state.palette_index = 0;
}

/// Try to extract an inline shortcut command from a normal message.
/// Returns the command line to execute and the remaining chat text.
pub fn extract_inline_command(text: &str) -> (Option<String>, String) {
    let words: Vec<_> = text.split_whitespace().collect();
    for (idx, word) in words.iter().enumerate() {
        let name = word.strip_prefix('/').unwrap_or(word);
        if INLINE_SHORTCUTS.contains(&name) {
            let remaining: Vec<_> = words
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != idx)
                .map(|(_, w)| *w)
                .collect();
            return (Some(format!("/{}", name)), remaining.join(" "));
        }
    }
    (None, text.to_string())
}

/// Build a small fallback catalog for offline command palette hints.
pub fn fallback_commands() -> Vec<crate::tui::state::CommandInfo> {
    vec![
        cmd("new", "new", "Create a new session", "[model]", "session", "essential", true),
        cmd("clear", "clear", "Clear chat history", "", "session", "essential", true),
        cmd("help", "help", "Show command help", "[page]", "status", "essential", false),
        cmd("status", "status", "Gateway status", "", "status", "essential", false),
        cmd("tools", "tools", "List available tools", "[compact|verbose]", "status", "standard", false),
        cmd("model", "model", "Set default model", "<id|status>", "model", "essential", false),
        cmd("usage", "usage", "Show usage statistics", "[off|tokens|full|cost]", "status", "standard", false),
        cmd("subagents", "subagents", "Manage sub-agents", "<subcommand>", "agents", "power", false),
        cmd("acp", "acp", "Manage ACP sessions", "<subcommand>", "agents", "power", false),
        cmd("mcp", "mcp", "Manage MCP servers", "<subcommand>", "admin", "power", false),
        cmd("config", "config", "Manage runtime config", "<subcommand>", "admin", "power", false),
        cmd("restart", "restart", "Restart the gateway", "", "admin", "power", false),
        cmd("bash", "bash", "Run a shell command", "<command>", "admin", "power", false),
    ]
}

fn cmd(
    key: &str,
    name: &str,
    description: &str,
    usage: &str,
    category: &str,
    tier: &str,
    local: bool,
) -> crate::tui::state::CommandInfo {
    crate::tui::state::CommandInfo {
        key: key.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        usage: usage.to_string(),
        category: category.to_string(),
        tier: tier.to_string(),
        local,
        requires_admin: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_with_args() {
        assert_eq!(parse_slash_command("/model gpt-4"), Some(("model", "gpt-4")));
    }

    #[test]
    fn parse_command_without_args() {
        assert_eq!(parse_slash_command("/new"), Some(("new", "")));
    }

    #[test]
    fn parse_non_command() {
        assert_eq!(parse_slash_command("hello"), None);
    }

    #[test]
    fn extract_inline_shortcut() {
        let (cmd, remaining) = extract_inline_command("Hey /whoami thanks");
        assert_eq!(cmd, Some("/whoami".to_string()));
        assert_eq!(remaining, "Hey thanks");
    }

    #[test]
    fn no_inline_shortcut_for_unknown_command() {
        let (cmd, remaining) = extract_inline_command("Hey /restart thanks");
        assert_eq!(cmd, None);
        assert_eq!(remaining, "Hey /restart thanks");
    }
}
