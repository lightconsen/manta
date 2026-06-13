//! Central async event loop merging input, network, and rendering.

use crate::tui::actions::TuiAction;
use crate::tui::commands::{action_for_input, handle_slash_command};
use crate::tui::state::{AppState, InputMode, Popup, SessionSummary};
use crate::tui::ui;
use crate::tui::ws_client::{ClientEvent, WsClient, WsMessage};
use ratatui::Terminal;
use serde_json::Value;
use std::io::Stdout;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::time::Interval;

/// Run the main event loop until the user quits or a fatal error occurs.
pub async fn run(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
    state: Arc<RwLock<AppState>>,
    mut ws_client: WsClient,
    mut input_rx: mpsc::UnboundedReceiver<TuiAction>,
    mut render_interval: Interval,
) -> Result<(), crate::tui::error::TuiError> {
    // Fetch initial session list.
    let _ = fetch_sessions(&mut ws_client, Arc::clone(&state)).await;

    loop {
        tokio::select! {
            Some(action) = input_rx.recv() => {
                let action = {
                    let s = state.read().await;
                    action_for_input(action, &s.input_buffer)
                };
                let should_break = handle_action(action, Arc::clone(&state), &mut ws_client).await?;
                if should_break {
                    break;
                }
            }

            Some(msg) = ws_client.next() => {
                handle_network_message(msg, Arc::clone(&state), &mut ws_client).await;
            }

            _ = render_interval.tick() => {
                {
                    let mut s = state.write().await;
                    s.clear_expired_toasts();
                    if s.terminal_size == (0, 0) {
                        s.terminal_size = terminal.size().map(|r| (r.width, r.height)).unwrap_or((80, 24));
                    }
                }
                let guard = state.read().await;
                terminal.draw(|f| ui::render(f, &guard))?;
            }
        }

        if state.read().await.should_quit {
            break;
        }
        if let Some(err) = &state.read().await.fatal_error.clone() {
            return Err(crate::tui::error::TuiError::WebSocket(err.clone()));
        }
    }

    Ok(())
}

/// Handle a user action. Returns `true` if the loop should exit.
async fn handle_action(
    action: TuiAction,
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
) -> Result<bool, crate::tui::error::TuiError> {
    let mut s = state.write().await;

    // In config-edit mode, keys edit the current value.
    if s.input_mode == InputMode::ConfigEdit {
        match action {
            TuiAction::SendMessage => {
                let value = s.input_buffer.trim().to_string();
                s.input_buffer.clear();
                let key = config_keys()
                    .get(s.config_selected_index)
                    .cloned()
                    .unwrap_or_default();
                s.config_edits.insert(key, value);
                s.input_mode = InputMode::Popup;
            }
            TuiAction::SaveConfig => {
                s.input_mode = InputMode::Popup;
                drop(s);
                save_config_edits(Arc::clone(&state), ws_client).await.ok();
                return Ok(false);
            }
            TuiAction::InputChar(c) => s.input_buffer.push(c),
            TuiAction::InputBackspace => {
                s.input_buffer.pop();
            }
            TuiAction::ClosePopup => {
                s.input_mode = InputMode::Popup;
                s.input_buffer.clear();
            }
            _ => {}
        }
        return Ok(false);
    }

    match action {
        TuiAction::Quit => {
            s.should_quit = true;
            return Ok(true);
        }
        TuiAction::Resize(cols, rows) => {
            s.terminal_size = (cols, rows);
        }
        TuiAction::SendMessage => {
            let text = s.input_buffer.trim().to_string();
            if text.is_empty() {
                return Ok(false);
            }
            s.input_buffer.clear();
            drop(s);

            if text.starts_with('/') {
                return handle_slash_command(&text, state, ws_client)
                    .await
                    .map(|_| false);
            }

            send_chat_message(state, ws_client, text).await?;
        }
        TuiAction::RunSlashCommand(cmd) => {
            drop(s);
            handle_slash_command(&cmd, state, ws_client).await?;
        }
        TuiAction::InputChar(c) => match s.input_mode {
            InputMode::Normal | InputMode::ConfigEdit => s.input_buffer.push(c),
            InputMode::Popup => {}
        },
        TuiAction::InputBackspace => {
            s.input_buffer.pop();
        }
        TuiAction::CursorLeft => {
            // Cursor always stays at end for simplicity.
        }
        TuiAction::CursorRight => {}
        TuiAction::CursorHome => {}
        TuiAction::CursorEnd => {}
        TuiAction::ScrollUp => {
            s.scroll_offset = s.scroll_offset.saturating_add(3);
        }
        TuiAction::ScrollDown => {
            s.scroll_offset = s.scroll_offset.saturating_sub(3);
        }
        TuiAction::FocusNext | TuiAction::FocusPrevious => {
            // Focus cycling is simplified; input is always focused.
        }
        TuiAction::OpenHelp => {
            s.popup = Popup::Help;
            s.input_mode = InputMode::Popup;
            drop(s);
            let _ = fetch_commands(ws_client, Arc::clone(&state)).await;
        }
        TuiAction::ClosePopup => {
            s.popup = Popup::None;
            s.input_mode = InputMode::Normal;
        }
        TuiAction::OpenConfigEditor => {
            s.popup = Popup::ConfigEditor;
            s.input_mode = InputMode::Popup;
            drop(s);
            let _ = fetch_config(ws_client, Arc::clone(&state)).await;
        }
        TuiAction::SaveConfig => {
            if s.popup == Popup::ConfigEditor {
                drop(s);
                save_config_edits(Arc::clone(&state), ws_client).await.ok();
            }
        }
        TuiAction::SelectUp => match s.popup {
            Popup::None => {
                if !s.sessions.is_empty() {
                    s.selected_session_index = s.selected_session_index.saturating_sub(1);
                }
            }
            Popup::ConfigEditor => {
                s.config_selected_index = s.config_selected_index.saturating_sub(1);
            }
            _ => {}
        },
        TuiAction::SelectDown => match s.popup {
            Popup::None => {
                if !s.sessions.is_empty() {
                    s.selected_session_index =
                        (s.selected_session_index + 1).min(s.sessions.len() - 1);
                }
            }
            Popup::ConfigEditor => {
                let keys = config_keys();
                s.config_selected_index =
                    (s.config_selected_index + 1).min(keys.len().saturating_sub(1));
            }
            _ => {}
        },
        TuiAction::SelectEnter => match s.popup {
            Popup::None => {
                if let Some(session) = s.sessions.get(s.selected_session_index).cloned() {
                    let sid = session.id.clone();
                    s.select_session(&sid);
                    drop(s);
                    subscribe_session(ws_client, &sid).await.ok();
                }
            }
            Popup::ConfigEditor => {
                s.input_mode = InputMode::ConfigEdit;
                let key = config_keys()
                    .get(s.config_selected_index)
                    .cloned()
                    .unwrap_or_default();
                let current = s.config_cache.get(&key).cloned().unwrap_or(Value::Null);
                s.input_buffer = current.to_string();
            }
            _ => {}
        },
        TuiAction::NewSession => {
            drop(s);
            create_session(state, ws_client).await?;
        }
        TuiAction::DeleteSelected => {
            let idx = s.selected_session_index;
            if let Some(session) = s.sessions.get(idx).cloned() {
                drop(s);
                delete_session(state, ws_client, &session.id).await?;
            }
        }
        TuiAction::Approve { id } => {
            drop(s);
            let _ = ws_client
                .request(
                    "approval.respond",
                    Some(serde_json::json!({ "approval_id": id, "approved": true })),
                )
                .await;
        }
        TuiAction::Reject { id } => {
            drop(s);
            let _ = ws_client
                .request(
                    "approval.respond",
                    Some(serde_json::json!({ "approval_id": id, "approved": false })),
                )
                .await;
        }
        TuiAction::None => {}
    }

    Ok(false)
}

/// Send a chat message to the gateway.
async fn send_chat_message(
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
    text: String,
) -> Result<(), crate::tui::error::TuiError> {
    let session_id = {
        let mut s = state.write().await;
        let sid = s
            .current_session
            .clone()
            .unwrap_or_else(|| format!("tui:{}", uuid::Uuid::new_v4()));
        s.current_session = Some(sid.clone());
        s.ensure_session(&sid);
        s.select_session(&sid);
        let msg_id = format!("msg_{}", s.messages.len());
        s.append_user_message(msg_id, &text);
        sid
    };

    ws_client
        .request(
            "chat.send",
            Some(serde_json::json!({
                "message": text,
                "session_id": session_id,
            })),
        )
        .await?;

    Ok(())
}

/// Handle an incoming WebSocket message.
async fn handle_network_message(
    msg: WsMessage,
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
) {
    match msg {
        WsMessage::Event(event) => handle_event(event, Arc::clone(&state), ws_client).await,
        WsMessage::OrphanResponse(resp) => {
            if !resp.ok {
                let mut s = state.write().await;
                if let Some(err) = resp.error {
                    s.error_toast(format!("{}: {}", err.code, err.message));
                }
            }
        }
    }
}

/// Handle a gateway event.
async fn handle_event(event: ClientEvent, state: Arc<RwLock<AppState>>, _ws_client: &mut WsClient) {
    let mut s = state.write().await;

    match event.event.as_str() {
        "chat.delta" => {
            if let Some(payload) = event.payload {
                if let Some(content) = payload.get("content").and_then(|v| v.as_str()) {
                    s.append_delta(content);
                }
            }
        }
        "agent.thinking" => {
            if let Some(payload) = event.payload {
                if let Some(content) = payload.get("content").and_then(|v| v.as_str()) {
                    if let Some(msg) = s.last_streaming_message() {
                        if let Some(ref mut thinking) = msg.thinking {
                            thinking.push_str(content);
                        } else {
                            msg.thinking = Some(content.to_string());
                        }
                    }
                }
            }
        }
        "tool.calling" => {
            if let Some(payload) = event.payload {
                let tool_name = payload
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let msg_id = format!("tool_call_{}", s.messages.len());
                s.append_system_message(msg_id, format!("Calling {}...", tool_name));
            }
        }
        "tool.result" => {
            if let Some(payload) = event.payload {
                let tool_name = payload
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool")
                    .to_string();
                let result = payload
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("done")
                    .to_string();
                let msg_id = format!("tool_result_{}", s.messages.len());
                s.append_system_message(msg_id, format!("{}: {}", tool_name, result));
            }
        }
        "chat.final" => {
            let content = event
                .payload
                .as_ref()
                .and_then(|p| p.get("response"))
                .and_then(|v| v.as_str());
            s.finalize_assistant(content);
        }
        "chat.error" => {
            let message = event
                .payload
                .as_ref()
                .and_then(|p| p.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("chat error")
                .to_string();
            s.error_assistant(&message);
            s.error_toast(message);
        }
        "session.created" => {
            if let Some(payload) = event.payload {
                if let Some(sid) = payload.get("session_id").and_then(|v| v.as_str()) {
                    s.ensure_session(sid);
                    s.toast(format!("Session created: {}", sid));
                }
            }
        }
        "session.renamed" => {
            if let Some(payload) = event.payload {
                if let (Some(sid), Some(name)) = (
                    payload.get("session_id").and_then(|v| v.as_str()),
                    payload.get("name").and_then(|v| v.as_str()),
                ) {
                    if let Some(session) = s.sessions.iter_mut().find(|x| x.id == sid) {
                        session.label = Some(name.to_string());
                    }
                }
            }
        }
        "approval.required" => {
            if let Some(payload) = event.payload {
                let tool_name = payload
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tool");
                s.toast(format!("Approval required: {}", tool_name));
            }
        }
        "channel.status" | "agent.status" => {
            // Ignored for now.
        }
        _ => {
            // Other events ignored.
        }
    }
}

/// Fetch the list of sessions from the gateway.
async fn fetch_sessions(
    ws_client: &mut WsClient,
    state: Arc<RwLock<AppState>>,
) -> Result<(), crate::tui::error::TuiError> {
    let result = ws_client.request("sessions.list", None).await?;
    let mut s = state.write().await;
    if let Some(items) = result.as_array() {
        s.sessions = items
            .iter()
            .filter_map(|v| {
                Some(SessionSummary {
                    id: v.get("id")?.as_str()?.to_string(),
                    label: v
                        .get("name")
                        .or_else(|| v.get("label"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    agent_id: v.get("agent_id").and_then(|v| v.as_str()).map(String::from),
                    selected: false,
                })
            })
            .collect();
        if let Some(current) = s.current_session.clone() {
            s.select_session(&current);
        }
    }
    Ok(())
}

/// Subscribe to a session.
async fn subscribe_session(
    ws_client: &mut WsClient,
    session_id: &str,
) -> Result<(), crate::tui::error::TuiError> {
    ws_client
        .request("sessions.subscribe", Some(serde_json::json!({ "session_id": session_id })))
        .await?;
    Ok(())
}

/// Create a new session and select it.
async fn create_session(
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
) -> Result<(), crate::tui::error::TuiError> {
    let result = ws_client.request("sessions.create", None).await?;
    if let Some(sid) = result.get("session_id").and_then(|v| v.as_str()) {
        {
            let mut s = state.write().await;
            s.ensure_session(sid);
            s.select_session(sid);
        }
        subscribe_session(ws_client, sid).await?;
    }
    Ok(())
}

/// Delete a session.
async fn delete_session(
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
    session_id: &str,
) -> Result<(), crate::tui::error::TuiError> {
    ws_client
        .request("sessions.delete", Some(serde_json::json!({ "session_id": session_id })))
        .await?;
    {
        let mut s = state.write().await;
        s.sessions.retain(|s| s.id != session_id);
        if s.current_session.as_deref() == Some(session_id) {
            s.current_session = None;
            s.messages.clear();
        }
        if s.selected_session_index >= s.sessions.len() {
            s.selected_session_index = s.sessions.len().saturating_sub(1);
        }
    }
    Ok(())
}

/// Fetch the command list for the help popup.
pub(crate) async fn fetch_commands(
    ws_client: &mut WsClient,
    state: Arc<RwLock<AppState>>,
) -> Result<(), crate::tui::error::TuiError> {
    let result = ws_client
        .request("commands.list", Some(serde_json::json!({ "tier": "essential" })))
        .await?;
    let mut s = state.write().await;
    if let Some(items) = result.as_array() {
        s.command_list = items
            .iter()
            .filter_map(|v| {
                Some(crate::tui::state::CommandInfo {
                    name: v.get("name")?.as_str()?.to_string(),
                    description: v
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    usage: v
                        .get("usage")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
            })
            .collect();
    }
    Ok(())
}

/// Fetch editable config values.
pub(crate) async fn fetch_config(
    ws_client: &mut WsClient,
    state: Arc<RwLock<AppState>>,
) -> Result<(), crate::tui::error::TuiError> {
    for key in config_keys() {
        if let Ok(value) = ws_client
            .request("config.get", Some(serde_json::json!({ "key": key })))
            .await
        {
            let mut s = state.write().await;
            s.config_cache.insert(key, value);
        }
    }
    Ok(())
}

/// Save pending config edits.
pub async fn save_config_edits(
    state: Arc<RwLock<AppState>>,
    ws_client: &mut WsClient,
) -> Result<(), crate::tui::error::TuiError> {
    let edits = {
        let s = state.read().await;
        s.config_edits.clone()
    };

    for (key, value) in edits {
        ws_client
            .request("config.set", Some(serde_json::json!({ "key": key, "value": value })))
            .await?;
    }

    {
        let mut s = state.write().await;
        s.config_edits.clear();
        s.popup = Popup::None;
        s.input_mode = InputMode::Normal;
        s.toast("Config updated. Restart required for some changes.");
    }

    fetch_config(ws_client, Arc::clone(&state)).await?;
    Ok(())
}

/// Keys exposed in the config editor.
pub fn config_keys() -> Vec<String> {
    vec![
        "gateway.host".to_string(),
        "gateway.port".to_string(),
        "logging.level".to_string(),
        "model.default".to_string(),
        "model.provider".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_keys_listed() {
        let keys = config_keys();
        assert!(keys.contains(&"gateway.host".to_string()));
    }
}
