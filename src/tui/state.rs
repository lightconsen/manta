//! Central application state for the TUI.

use chrono::{DateTime, Local};
use serde_json::Value;
use std::collections::HashMap;

/// Connection state of the TUI to the gateway.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    /// Disconnected, ready to reconnect.
    #[default]
    Disconnected,
    /// TCP / WebSocket handshake in progress.
    Connecting,
    /// Connected and handshake complete.
    Connected {
        /// Features advertised by the server.
        features: Vec<String>,
        /// Scopes granted to this connection.
        scopes_granted: Vec<String>,
        /// Server version string.
        server_version: String,
    },
    /// Recoverable error state with a human-readable message.
    Error(String),
}

/// Status of an in-flight or completed chat message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MessageStatus {
    /// Message is being sent.
    Sending,
    /// Assistant response is streaming in.
    Streaming,
    /// Message is complete.
    #[default]
    Complete,
    /// An error occurred while generating the response.
    Error(String),
}

/// A chat message rendered in the TUI.
#[derive(Debug, Clone, Default)]
pub struct ChatMessage {
    /// Unique message id.
    pub id: String,
    /// Role: "user", "assistant", "system", or "tool".
    pub role: String,
    /// Renderable text content.
    pub content: String,
    /// Optional reasoning / thinking text.
    pub thinking: Option<String>,
    /// Optional tool name when role is "tool".
    pub tool_name: Option<String>,
    /// Message status.
    pub status: MessageStatus,
    /// Timestamp.
    pub timestamp: DateTime<Local>,
    /// Extra metadata (duration, tool count, etc.).
    pub metadata: Option<Value>,
}

/// Summary of a session shown in the sidebar.
#[derive(Debug, Clone, Default)]
pub struct SessionSummary {
    /// Session id.
    pub id: String,
    /// Human-readable label, if any.
    pub label: Option<String>,
    /// Agent id for the session.
    pub agent_id: Option<String>,
    /// Whether the session is currently selected.
    pub selected: bool,
}

/// Which UI element has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Normal chat input.
    #[default]
    Normal,
    /// Editing a config value.
    ConfigEdit,
    /// Navigating a popup.
    Popup,
}

/// A transient toast notification.
#[derive(Debug, Clone)]
pub struct Toast {
    /// Message text.
    pub message: String,
    /// Creation timestamp.
    pub created_at: DateTime<Local>,
    /// Seconds to live.
    pub ttl_seconds: u64,
    /// Whether this is an error toast.
    pub is_error: bool,
}

impl Toast {
    /// Create a new toast.
    pub fn new(message: impl Into<String>, ttl_seconds: u64, is_error: bool) -> Self {
        Self {
            message: message.into(),
            created_at: Local::now(),
            ttl_seconds,
            is_error,
        }
    }
}

/// Active popup overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Popup {
    /// No popup.
    #[default]
    None,
    /// Help popup.
    Help,
    /// Config editor popup.
    ConfigEditor,
}

/// Central mutable application state.
#[derive(Debug, Default)]
pub struct AppState {
    /// Current connection state.
    pub connection: ConnectionState,
    /// Terminal size (width, height).
    pub terminal_size: (u16, u16),
    /// Currently selected session id.
    pub current_session: Option<String>,
    /// Sessions known to the client.
    pub sessions: Vec<SessionSummary>,
    /// Chat messages for the current session.
    pub messages: Vec<ChatMessage>,
    /// Current input buffer.
    pub input_buffer: String,
    /// Current input mode.
    pub input_mode: InputMode,
    /// Current popup, if any.
    pub popup: Popup,
    /// Scroll offset for the chat panel (lines from bottom).
    pub scroll_offset: usize,
    /// Index of the selected session in the sidebar.
    pub selected_session_index: usize,
    /// Transient toasts.
    pub toasts: Vec<Toast>,
    /// Cached config values for the config editor.
    pub config_cache: HashMap<String, Value>,
    /// Pending config edits (key -> new value).
    pub config_edits: HashMap<String, String>,
    /// Index of selected config row in the editor.
    pub config_selected_index: usize,
    /// Cached command list for the help popup.
    pub command_list: Vec<CommandInfo>,
    /// Whether a response is currently streaming.
    pub is_running: bool,
    /// Fatal error that should end the TUI.
    pub fatal_error: Option<String>,
    /// Whether the app should quit on next loop iteration.
    pub should_quit: bool,
}

/// Information about a gateway command shown in the help popup.
#[derive(Debug, Clone, Default)]
pub struct CommandInfo {
    /// Command name (e.g. "new").
    pub name: String,
    /// Short description.
    pub description: String,
    /// Usage pattern.
    pub usage: String,
}

impl AppState {
    /// Append a user message and return its id.
    pub fn append_user_message(
        &mut self,
        id: impl Into<String>,
        content: impl Into<String>,
    ) -> String {
        let id = id.into();
        self.messages.push(ChatMessage {
            id: id.clone(),
            role: "user".to_string(),
            content: content.into(),
            status: MessageStatus::Complete,
            timestamp: Local::now(),
            ..ChatMessage::default()
        });
        id
    }

    /// Append an empty assistant message and return its id.
    pub fn append_assistant_message(&mut self, id: impl Into<String>) -> String {
        let id = id.into();
        self.messages.push(ChatMessage {
            id: id.clone(),
            role: "assistant".to_string(),
            status: MessageStatus::Streaming,
            timestamp: Local::now(),
            ..ChatMessage::default()
        });
        self.is_running = true;
        id
    }

    /// Append a system/tool message.
    pub fn append_system_message(&mut self, id: impl Into<String>, content: impl Into<String>) {
        self.messages.push(ChatMessage {
            id: id.into(),
            role: "system".to_string(),
            content: content.into(),
            status: MessageStatus::Complete,
            timestamp: Local::now(),
            ..ChatMessage::default()
        });
    }

    /// Find the last assistant message that is still streaming.
    pub fn last_streaming_message(&mut self) -> Option<&mut ChatMessage> {
        self.messages
            .iter_mut()
            .rev()
            .find(|m| m.role == "assistant" && matches!(m.status, MessageStatus::Streaming))
    }

    /// Update or create an assistant streaming message with a delta.
    pub fn append_delta(&mut self, content: &str) {
        if let Some(msg) = self.last_streaming_message() {
            msg.content.push_str(content);
        } else {
            self.append_assistant_message(format!("assistant_{}", self.messages.len()));
            if let Some(msg) = self.last_streaming_message() {
                msg.content.push_str(content);
            }
        }
    }

    /// Finalize the current streaming assistant message.
    pub fn finalize_assistant(&mut self, content: Option<&str>) {
        if let Some(msg) = self.last_streaming_message() {
            if let Some(c) = content {
                msg.content = c.to_string();
            }
            msg.status = MessageStatus::Complete;
        }
        self.is_running = false;
    }

    /// Mark the current streaming message as errored.
    pub fn error_assistant(&mut self, message: &str) {
        if let Some(msg) = self.last_streaming_message() {
            msg.status = MessageStatus::Error(message.to_string());
        }
        self.is_running = false;
    }

    /// Add a non-fatal toast.
    pub fn toast(&mut self, message: impl Into<String>) {
        self.toasts.push(Toast::new(message, 5, false));
    }

    /// Add an error toast.
    pub fn error_toast(&mut self, message: impl Into<String>) {
        self.toasts.push(Toast::new(message, 8, true));
    }

    /// Clear expired toasts.
    pub fn clear_expired_toasts(&mut self) {
        let now = Local::now();
        self.toasts.retain(|t| {
            let elapsed = now.signed_duration_since(t.created_at).num_seconds().max(0) as u64;
            elapsed < t.ttl_seconds
        });
    }

    /// Return true if the current session is in the list, creating a placeholder if needed.
    pub fn ensure_session(&mut self, session_id: &str) {
        if !self.sessions.iter().any(|s| s.id == session_id) {
            self.sessions.push(SessionSummary {
                id: session_id.to_string(),
                label: None,
                agent_id: None,
                selected: false,
            });
        }
    }

    /// Select a session by id.
    pub fn select_session(&mut self, session_id: &str) {
        self.current_session = Some(session_id.to_string());
        for (idx, s) in self.sessions.iter_mut().enumerate() {
            s.selected = s.id == session_id;
            if s.selected {
                self.selected_session_index = idx;
            }
        }
    }

    /// Return true if the granted scopes include `scope`.
    pub fn has_scope(&self, scope: &str) -> bool {
        matches!(
            &self.connection,
            ConnectionState::Connected { scopes_granted, .. }
                if scopes_granted.iter().any(|s| s == scope)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_finalize_delta() {
        let mut state = AppState::default();
        state.append_delta("Hello");
        state.append_delta(" world");
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].content, "Hello world");
        state.finalize_assistant(None);
        assert!(matches!(state.messages[0].status, MessageStatus::Complete));
        assert!(!state.is_running);
    }

    #[test]
    fn toasts_expire() {
        let mut state = AppState::default();
        state.toast("hello");
        assert_eq!(state.toasts.len(), 1);
        // Simulate expiration by setting created_at far in the past.
        state.toasts[0].created_at = Local::now() - chrono::Duration::seconds(10);
        state.clear_expired_toasts();
        assert!(state.toasts.is_empty());
    }

    #[test]
    fn scope_check() {
        let mut state = AppState::default();
        assert!(!state.has_scope("write"));
        state.connection = ConnectionState::Connected {
            features: vec![],
            scopes_granted: vec!["chat".to_string(), "write".to_string()],
            server_version: "0.1.2".to_string(),
        };
        assert!(state.has_scope("write"));
    }
}
