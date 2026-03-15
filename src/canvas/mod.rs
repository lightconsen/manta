//! Canvas/A2UI - Dynamic UI Generation System for Manta
//!
//! Provides OpenClaw-compatible A2UI (Agent-to-UI) capabilities for generating
//! dynamic user interfaces through WebSocket updates. Supports forms, buttons,
//! progress indicators, and real-time content streaming.

use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Unique identifier for a UI session
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CanvasId(pub String);

impl CanvasId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for CanvasId {
    fn default() -> Self {
        Self::new()
    }
}

/// A2UI Component types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum CanvasComponent {
    /// Container for other components
    Container {
        id: String,
        children: Vec<CanvasComponent>,
        layout: Option<ContainerLayout>,
    },
    /// Text display
    Text {
        id: String,
        content: String,
        style: Option<TextStyle>,
    },
    /// Markdown content
    Markdown {
        id: String,
        content: String,
    },
    /// Input field
    Input {
        id: String,
        label: Option<String>,
        placeholder: Option<String>,
        value: Option<String>,
        input_type: Option<String>,
        required: Option<bool>,
    },
    /// Textarea for multi-line input
    Textarea {
        id: String,
        label: Option<String>,
        placeholder: Option<String>,
        value: Option<String>,
        rows: Option<u32>,
    },
    /// Button
    Button {
        id: String,
        label: String,
        variant: Option<String>,
        disabled: Option<bool>,
    },
    /// Select dropdown
    Select {
        id: String,
        label: Option<String>,
        options: Vec<SelectOption>,
        value: Option<String>,
    },
    /// Checkbox
    Checkbox {
        id: String,
        label: String,
        checked: Option<bool>,
    },
    /// Radio button group
    RadioGroup {
        id: String,
        label: Option<String>,
        options: Vec<SelectOption>,
        value: Option<String>,
    },
    /// Progress bar
    Progress {
        id: String,
        value: f64,
        max: Option<f64>,
        label: Option<String>,
    },
    /// Spinner/loading indicator
    Spinner {
        id: String,
        label: Option<String>,
    },
    /// Image display
    Image {
        id: String,
        src: String,
        alt: Option<String>,
    },
    /// Code block with syntax highlighting
    Code {
        id: String,
        content: String,
        language: Option<String>,
    },
    /// Table display
    Table {
        id: String,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Divider line
    Divider {
        id: String,
    },
    /// Alert/notification
    Alert {
        id: String,
        level: String,
        message: String,
    },
}

/// Container layout options
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerLayout {
    Vertical,
    Horizontal,
    Grid { columns: u32 },
}

/// Text styling options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextStyle {
    pub size: Option<String>,
    pub weight: Option<String>,
    pub color: Option<String>,
}

/// Select option
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

/// User interaction event from Canvas
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum CanvasEvent {
    /// Button clicked
    ButtonClick { component_id: String },
    /// Input value changed
    InputChange { component_id: String, value: String },
    /// Select option changed
    SelectChange { component_id: String, value: String },
    /// Checkbox toggled
    CheckboxChange { component_id: String, checked: bool },
    /// Radio selection changed
    RadioChange { component_id: String, value: String },
    /// Form submitted
    FormSubmit { component_id: String, values: HashMap<String, Value> },
    /// Canvas closed by user
    Close,
}

/// Canvas update message (sent to clients)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum CanvasUpdate {
    /// Initialize/replace entire canvas
    Init { canvas_id: String, root: CanvasComponent },
    /// Update specific component
    Update { component_id: String, component: CanvasComponent },
    /// Remove component
    Remove { component_id: String },
    /// Append child to container
    Append { parent_id: String, component: CanvasComponent },
    /// Show alert/notification
    Notify { level: String, message: String },
    /// Close canvas
    Close,
}

/// Canvas session state
pub struct CanvasSession {
    pub id: CanvasId,
    pub root: RwLock<CanvasComponent>,
    pub event_tx: mpsc::Sender<CanvasEvent>,
    pub update_tx: broadcast::Sender<CanvasUpdate>,
}

impl CanvasSession {
    pub fn new(event_tx: mpsc::Sender<CanvasEvent>) -> Self {
        let id = CanvasId::new();
        let (update_tx, _) = broadcast::channel(100);

        Self {
            id: id.clone(),
            root: RwLock::new(CanvasComponent::Container {
                id: "root".to_string(),
                children: vec![],
                layout: Some(ContainerLayout::Vertical),
            }),
            event_tx,
            update_tx,
        }
    }

    /// Initialize canvas with root component
    pub async fn init(&self, root: CanvasComponent) {
        let mut guard = self.root.write().await;
        *guard = root.clone();

        let _ = self.update_tx.send(CanvasUpdate::Init {
            canvas_id: self.id.0.clone(),
            root,
        });
    }

    /// Update a specific component
    pub async fn update(&self, component_id: String, component: CanvasComponent) {
        let _ = self.update_tx.send(CanvasUpdate::Update {
            component_id,
            component,
        });
    }

    /// Append component to container
    pub async fn append(&self, parent_id: String, component: CanvasComponent) {
        let _ = self.update_tx.send(CanvasUpdate::Append {
            parent_id,
            component,
        });
    }

    /// Show notification
    pub async fn notify(&self, level: String, message: String) {
        let _ = self.update_tx.send(CanvasUpdate::Notify { level, message });
    }

    /// Close canvas
    pub async fn close(&self) {
        let _ = self.update_tx.send(CanvasUpdate::Close);
    }
}

/// Canvas manager handles multiple UI sessions
pub struct CanvasManager {
    sessions: RwLock<HashMap<CanvasId, Arc<CanvasSession>>>,
}

impl CanvasManager {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Create new canvas session
    pub async fn create_session(&self, event_tx: mpsc::Sender<CanvasEvent>) -> Arc<CanvasSession> {
        let session = Arc::new(CanvasSession::new(event_tx));
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id.clone(), session.clone());
        session
    }

    /// Get session by ID
    pub async fn get_session(&self, id: &CanvasId) -> Option<Arc<CanvasSession>> {
        let sessions = self.sessions.read().await;
        sessions.get(id).cloned()
    }

    /// Remove session
    pub async fn remove_session(&self, id: &CanvasId) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(id);
    }

    /// List active sessions
    pub async fn list_sessions(&self) -> Vec<CanvasId> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
    }
}

impl Default for CanvasManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Canvas protocol handler for WebSocket
pub struct CanvasWebSocketHandler {
    canvas_id: CanvasId,
    event_tx: mpsc::Sender<CanvasEvent>,
    update_rx: broadcast::Receiver<CanvasUpdate>,
}

impl CanvasWebSocketHandler {
    pub fn new(
        canvas_id: CanvasId,
        event_tx: mpsc::Sender<CanvasEvent>,
        update_rx: broadcast::Receiver<CanvasUpdate>,
    ) -> Self {
        Self {
            canvas_id,
            event_tx,
            update_rx,
        }
    }

    /// Handle incoming WebSocket message
    pub async fn handle_message(&self, msg: Message) -> Option<CanvasEvent> {
        match msg {
            Message::Text(text) => {
                debug!("Canvas {} received message: {}", self.canvas_id.0, text);

                match serde_json::from_str::<CanvasEvent>(&text) {
                    Ok(event) => {
                        let _ = self.event_tx.send(event.clone()).await;
                        Some(event)
                    }
                    Err(e) => {
                        warn!("Failed to parse canvas event: {}", e);
                        None
                    }
                }
            }
            _ => None,
        }
    }

    /// Get next update to send to client
    pub async fn next_update(&mut self) -> Option<CanvasUpdate> {
        match self.update_rx.recv().await {
            Ok(update) => Some(update),
            Err(_) => None,
        }
    }

    /// Get canvas ID
    pub fn canvas_id(&self) -> &CanvasId {
        &self.canvas_id
    }
}

/// Helper functions for creating common UI patterns
pub mod helpers {
    use super::*;

    /// Create a simple form with inputs and submit button
    pub fn create_form(id: impl Into<String>, inputs: Vec<(String, String)>) -> CanvasComponent {
        let id = id.into();
        let mut children = vec![];

        for (input_id, label) in inputs {
            children.push(CanvasComponent::Input {
                id: format!("{}_{}", id, input_id),
                label: Some(label),
                placeholder: None,
                value: None,
                input_type: Some("text".to_string()),
                required: Some(true),
            });
        }

        children.push(CanvasComponent::Button {
            id: format!("{}_submit", id),
            label: "Submit".to_string(),
            variant: Some("primary".to_string()),
            disabled: Some(false),
        });

        CanvasComponent::Container {
            id,
            children,
            layout: Some(ContainerLayout::Vertical),
        }
    }

    /// Create a progress indicator
    pub fn create_progress(id: impl Into<String>, value: f64, label: Option<String>) -> CanvasComponent {
        CanvasComponent::Progress {
            id: id.into(),
            value,
            max: Some(100.0),
            label,
        }
    }

    /// Create an alert
    pub fn create_alert(id: impl Into<String>, level: impl Into<String>, message: impl Into<String>) -> CanvasComponent {
        CanvasComponent::Alert {
            id: id.into(),
            level: level.into(),
            message: message.into(),
        }
    }

    /// Create a button group
    pub fn create_button_group(id: impl Into<String>, labels: Vec<String>) -> CanvasComponent {
        let id = id.into();
        let children = labels
            .into_iter()
            .enumerate()
            .map(|(i, label)| CanvasComponent::Button {
                id: format!("{}_btn_{}", id, i),
                label,
                variant: Some("secondary".to_string()),
                disabled: Some(false),
            })
            .collect();

        CanvasComponent::Container {
            id,
            children,
            layout: Some(ContainerLayout::Horizontal),
        }
    }

    /// Create a code display with copy button
    pub fn create_code_block(id: impl Into<String>, content: impl Into<String>, language: Option<String>) -> CanvasComponent {
        let id = id.into();
        CanvasComponent::Container {
            id: id.clone(),
            children: vec![
                CanvasComponent::Code {
                    id: format!("{}_code", id),
                    content: content.into(),
                    language,
                },
                CanvasComponent::Button {
                    id: format!("{}_copy", id),
                    label: "Copy".to_string(),
                    variant: Some("ghost".to_string()),
                    disabled: Some(false),
                },
            ],
            layout: Some(ContainerLayout::Vertical),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canvas_id_generation() {
        let id1 = CanvasId::new();
        let id2 = CanvasId::new();
        assert_ne!(id1.0, id2.0);
    }

    #[test]
    fn test_component_serialization() {
        let component = CanvasComponent::Text {
            id: "test".to_string(),
            content: "Hello".to_string(),
            style: None,
        };

        let json = serde_json::to_string(&component).unwrap();
        assert!(json.contains("text"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_canvas_update_serialization() {
        let update = CanvasUpdate::Notify {
            level: "info".to_string(),
            message: "Test".to_string(),
        };

        let json = serde_json::to_string(&update).unwrap();
        assert!(json.contains("notify"));
        assert!(json.contains("info"));
    }

    #[test]
    fn test_helper_create_form() {
        let form = helpers::create_form("my_form", vec![
            ("name".to_string(), "Name".to_string()),
            ("email".to_string(), "Email".to_string()),
        ]);

        match form {
            CanvasComponent::Container { children, .. } => {
                assert_eq!(children.len(), 3); // 2 inputs + 1 button
            }
            _ => panic!("Expected container"),
        }
    }
}
