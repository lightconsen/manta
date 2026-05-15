//! Canvas Tool — A2UI Canvas Control
//!
//! OpenClaw-compatible tool for controlling dynamic UI canvases.
//! Wraps the existing CanvasManager to expose canvas operations to the agent.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::canvas::{CanvasComponent, CanvasId, CanvasManager, CanvasUpdate};

use super::{Tool, ToolContext, ToolExecutionResult};

/// Canvas control tool
pub struct CanvasTool {
    manager: Arc<CanvasManager>,
}

impl CanvasTool {
    pub fn new(manager: Arc<CanvasManager>) -> Self {
        Self { manager }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum CanvasAction {
    Present {
        session_id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        components: Vec<CanvasComponentArg>,
    },
    Hide {
        session_id: String,
    },
    Update {
        session_id: String,
        component_id: String,
        component: CanvasComponentArg,
    },
    Append {
        session_id: String,
        parent_id: String,
        component: CanvasComponentArg,
    },
    Snapshot {
        session_id: String,
    },
    Notify {
        session_id: String,
        level: String,
        message: String,
    },
    Reset {
        session_id: String,
    },
}

/// Simplified component argument for tool input
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
enum CanvasComponentArg {
    Container {
        id: String,
        #[serde(default)]
        children: Vec<CanvasComponentArg>,
        #[serde(default)]
        layout: Option<String>,
    },
    Text {
        id: String,
        content: String,
    },
    Markdown {
        id: String,
        content: String,
    },
    Input {
        id: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
    },
    Button {
        id: String,
        label: String,
    },
    Progress {
        id: String,
        value: f64,
        #[serde(default)]
        max: Option<f64>,
        #[serde(default)]
        label: Option<String>,
    },
    Spinner {
        id: String,
        #[serde(default)]
        label: Option<String>,
    },
    Image {
        id: String,
        src: String,
    },
    Code {
        id: String,
        content: String,
        #[serde(default)]
        language: Option<String>,
    },
    Table {
        id: String,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Alert {
        id: String,
        level: String,
        message: String,
    },
}

impl CanvasComponentArg {
    fn into_component(self) -> CanvasComponent {
        match self {
            CanvasComponentArg::Container { id, children, layout } => CanvasComponent::Container {
                id,
                children: children.into_iter().map(|c| c.into_component()).collect(),
                layout: layout.and_then(|l| match l.as_str() {
                    "vertical" => Some(crate::canvas::ContainerLayout::Vertical),
                    "horizontal" => Some(crate::canvas::ContainerLayout::Horizontal),
                    "grid" => Some(crate::canvas::ContainerLayout::Grid { columns: 2 }),
                    _ => None,
                }),
            },
            CanvasComponentArg::Text { id, content } => CanvasComponent::Text {
                id,
                content,
                style: None,
            },
            CanvasComponentArg::Markdown { id, content } => CanvasComponent::Markdown { id, content },
            CanvasComponentArg::Input { id, label, placeholder } => CanvasComponent::Input {
                id,
                label,
                placeholder,
                value: None,
                input_type: None,
                required: None,
            },
            CanvasComponentArg::Button { id, label } => CanvasComponent::Button {
                id,
                label,
                variant: None,
                disabled: None,
            },
            CanvasComponentArg::Progress { id, value, max, label } => CanvasComponent::Progress {
                id,
                value,
                max,
                label,
            },
            CanvasComponentArg::Spinner { id, label } => CanvasComponent::Spinner { id, label },
            CanvasComponentArg::Image { id, src } => CanvasComponent::Image {
                id,
                src,
                alt: None,
            },
            CanvasComponentArg::Code { id, content, language } => CanvasComponent::Code {
                id,
                content,
                language,
            },
            CanvasComponentArg::Table { id, headers, rows } => CanvasComponent::Table {
                id,
                headers,
                rows,
            },
            CanvasComponentArg::Alert { id, level, message } => CanvasComponent::Alert {
                id,
                level,
                message,
            },
        }
    }
}

#[async_trait]
impl Tool for CanvasTool {
    fn name(&self) -> &str {
        "canvas"
    }

    fn description(&self) -> &str {
        "Control dynamic UI canvases (A2UI). Present content, update components, \
         show notifications, and take snapshots of agent-generated UIs."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["present", "hide", "update", "append", "snapshot", "notify", "reset"],
                    "description": "Canvas action"
                },
                "session_id": {
                    "type": "string",
                    "description": "Session/conversation ID for the canvas"
                },
                "title": {
                    "type": "string",
                    "description": "Title for present action"
                },
                "components": {
                    "type": "array",
                    "description": "Components to present",
                    "items": {
                        "type": "object",
                        "properties": {
                            "type": { "type": "string" },
                            "id": { "type": "string" }
                        }
                    }
                },
                "component_id": {
                    "type": "string",
                    "description": "Target component ID for update"
                },
                "parent_id": {
                    "type": "string",
                    "description": "Parent component ID for append"
                },
                "component": {
                    "type": "object",
                    "description": "Component definition"
                },
                "level": {
                    "type": "string",
                    "enum": ["info", "warn", "error", "success"],
                    "description": "Notification level"
                },
                "message": {
                    "type": "string",
                    "description": "Notification message"
                }
            },
            "required": ["action", "session_id"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let action: CanvasAction = match serde_json::from_value(args) {
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
            CanvasAction::Present {
                session_id,
                title,
                components,
            } => {
                let (_tx, _rx): (mpsc::Sender<crate::canvas::CanvasEvent>, _) = mpsc::channel(16);
                let session = self
                    .manager
                    .get_or_create_for_session(&session_id)
                    .await;

                let root = if let Some(title_text) = title {
                    CanvasComponent::Container {
                        id: "root".to_string(),
                        children: vec![
                            CanvasComponent::Text {
                                id: "title".to_string(),
                                content: title_text,
                                style: Some(crate::canvas::TextStyle {
                                    size: Some("20px".to_string()),
                                    weight: Some("bold".to_string()),
                                    color: None,
                                }),
                            },
                            CanvasComponent::Container {
                                id: "body".to_string(),
                                children: components.into_iter().map(|c| c.into_component()).collect(),
                                layout: Some(crate::canvas::ContainerLayout::Vertical),
                            },
                        ],
                        layout: Some(crate::canvas::ContainerLayout::Vertical),
                    }
                } else {
                    CanvasComponent::Container {
                        id: "root".to_string(),
                        children: components.into_iter().map(|c| c.into_component()).collect(),
                        layout: Some(crate::canvas::ContainerLayout::Vertical),
                    }
                };

                let update = CanvasUpdate::Init {
                    canvas_id: session_id.clone(),
                    root,
                };
                self.manager.apply_update(&session_id, update).await;

                info!("Canvas presented for session {}", session_id);
                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Canvas presented for session {}", session_id),
                    error: None,
                    data: Some(serde_json::json!({ "session_id": session_id })),
                    execution_time: start.elapsed(),
                })
            }
            CanvasAction::Hide { session_id } => {
                let update = CanvasUpdate::Close;
                self.manager.apply_update(&session_id, update).await;

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Canvas hidden for session {}", session_id),
                    error: None,
                    data: None,
                    execution_time: start.elapsed(),
                })
            }
            CanvasAction::Update {
                session_id,
                component_id,
                component,
            } => {
                let component = component.into_component();
                let update = CanvasUpdate::Update {
                    component_id,
                    component,
                };
                self.manager.apply_update(&session_id, update).await;

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Canvas updated for session {}", session_id),
                    error: None,
                    data: None,
                    execution_time: start.elapsed(),
                })
            }
            CanvasAction::Append {
                session_id,
                parent_id,
                component,
            } => {
                let component = component.into_component();
                let update = CanvasUpdate::Append {
                    parent_id,
                    component,
                };
                self.manager.apply_update(&session_id, update).await;

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Component appended for session {}", session_id),
                    error: None,
                    data: None,
                    execution_time: start.elapsed(),
                })
            }
            CanvasAction::Snapshot { session_id } => {
                let sessions = self.manager.list_sessions().await;
                let found = sessions.iter().any(|id| id.0 == session_id);

                Ok(ToolExecutionResult {
                    success: found,
                    output: if found {
                        format!("Canvas snapshot for session {}", session_id)
                    } else {
                        format!("No canvas found for session {}", session_id)
                    },
                    error: if found { None } else { Some("Canvas not found".to_string()) },
                    data: Some(serde_json::json!({
                        "session_id": session_id,
                        "active": found,
                        "total_sessions": sessions.len(),
                    })),
                    execution_time: start.elapsed(),
                })
            }
            CanvasAction::Notify {
                session_id,
                level,
                message,
            } => {
                let update = CanvasUpdate::Notify { level, message };
                self.manager.apply_update(&session_id, update).await;

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Notification sent to session {}", session_id),
                    error: None,
                    data: None,
                    execution_time: start.elapsed(),
                })
            }
            CanvasAction::Reset { session_id } => {
                let update = CanvasUpdate::Close;
                self.manager.apply_update(&session_id, update).await;

                let root = CanvasComponent::Container {
                    id: "root".to_string(),
                    children: vec![],
                    layout: Some(crate::canvas::ContainerLayout::Vertical),
                };
                let update = CanvasUpdate::Init {
                    canvas_id: session_id.clone(),
                    root,
                };
                self.manager.apply_update(&session_id, update).await;

                Ok(ToolExecutionResult {
                    success: true,
                    output: format!("Canvas reset for session {}", session_id),
                    error: None,
                    data: None,
                    execution_time: start.elapsed(),
                })
            }
        }
    }
}
