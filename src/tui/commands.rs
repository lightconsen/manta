//! Slash command parser and executor.

use crate::tui::actions::TuiAction;
use crate::tui::error::TuiError;
use crate::tui::event_loop::{fetch_commands, fetch_config};
use crate::tui::state::{AppState, InputMode, Popup};
use crate::tui::ws_client::WsClient;
use std::sync::Arc;
use tokio::sync::RwLock;

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
                            s.select_session(&current);
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
            s.select_session(sid);
            s.messages.clear();
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
}
