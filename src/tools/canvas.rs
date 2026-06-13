//! Canvas Tool — A2UI Canvas Control
//!
//! tool for controlling dynamic UI canvases.
//! Wraps the existing CanvasManager to expose canvas operations to the agent.

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

use crate::canvas::{CanvasComponent, CanvasManager, CanvasUpdate};

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
#[serde(tag = "action", rename_all = "snake_case")]
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

/// Resolve an image src to a displayable URL/data-URI.
///
/// - External URLs (`http://`, `https://`, `data:`) are returned as-is.
/// - Local file paths are read and encoded as base64 data URIs so the
/// frontend can display them without an extra HTTP request.
async fn resolve_image_src(src: &str, working_dir: &std::path::Path) -> String {
    if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
        return src.to_string();
    }

    let path = if std::path::Path::new(src).is_absolute() {
        std::path::PathBuf::from(src)
    } else {
        working_dir.join(src)
    };

    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mime = guess_mime_from_path(&path);
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            format!("data:{};base64,{}", mime, b64)
        }
        Err(e) => {
            tracing::warn!("Failed to read image for canvas at {:?}: {}", path, e);
            format!("(image not found: {})", src)
        }
    }
}

fn guess_mime_from_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "application/octet-stream",
    }
}

impl CanvasComponentArg {
    /// Recursively collect all image (id, src) pairs and inline SVG blocks.
    fn collect_images(&self, out: &mut Vec<(String, String)>) {
        match self {
            CanvasComponentArg::Container { children, .. } => {
                for child in children {
                    child.collect_images(out);
                }
            }
            CanvasComponentArg::Image { id, src } => {
                out.push((id.clone(), src.clone()));
            }
            CanvasComponentArg::Markdown { id, content } => {
                // Extract <svg>...</svg> blocks and convert them to base64 data URIs.
                let mut start = 0;
                let mut idx = 0;
                while let Some(svg_start) = content[start..].find("<svg") {
                    let abs_start = start + svg_start;
                    if let Some(svg_end) = content[abs_start..].find("</svg>") {
                        let abs_end = abs_start + svg_end + "</svg>".len();
                        let svg = &content[abs_start..abs_end];
                        let b64 = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
                        let data_uri = format!("data:image/svg+xml;base64,{}", b64);
                        out.push((format!("{}_svg_{}", id, idx), data_uri));
                        idx += 1;
                        start = abs_end;
                    } else {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

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
            CanvasComponentArg::Text { id, content } => {
                CanvasComponent::Text { id, content, style: None }
            }
            CanvasComponentArg::Markdown { id, content } => {
                CanvasComponent::Markdown { id, content }
            }
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
            CanvasComponentArg::Progress { id, value, max, label } => {
                CanvasComponent::Progress { id, value, max, label }
            }
            CanvasComponentArg::Spinner { id, label } => CanvasComponent::Spinner { id, label },
            CanvasComponentArg::Image { id, src } => CanvasComponent::Image { id, src, alt: None },
            CanvasComponentArg::Code { id, content, language } => {
                CanvasComponent::Code { id, content, language }
            }
            CanvasComponentArg::Table { id, headers, rows } => {
                CanvasComponent::Table { id, headers, rows }
            }
            CanvasComponentArg::Alert { id, level, message } => {
                CanvasComponent::Alert { id, level, message }
            }
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
            CanvasAction::Present { session_id, title, components } => {
                // Collect images from the component tree for inline display
                let mut images = Vec::new();
                for component in &components {
                    component.collect_images(&mut images);
                }

                if !images.is_empty() {
                    let mut markdown = String::new();
                    if let Some(ref t) = title {
                        markdown.push_str(&format!("**{}**\n\n", t));
                    }
                    for (id, src) in &images {
                        let resolved = resolve_image_src(src, &_context.working_directory).await;
                        markdown.push_str(&format!("![{}]({})\n\n", id, resolved));
                    }

                    info!("Canvas images presented inline for session {}", session_id);
                    return Ok(ToolExecutionResult {
                        success: true,
                        output: markdown.trim().to_string(),
                        error: None,
                        data: Some(serde_json::json!({
                            "session_id": session_id,
                            "images": images,
                            "inline": true,
                        })),
                        execution_time: start.elapsed(),
                    });
                }

                let (_tx, _rx): (mpsc::Sender<crate::canvas::CanvasEvent>, _) = mpsc::channel(16);
                let _session = self.manager.get_or_create_for_session(&session_id).await;

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
                                children: components
                                    .into_iter()
                                    .map(|c| c.into_component())
                                    .collect(),
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
                let update = CanvasUpdate::Update { component_id, component };
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
                let update = CanvasUpdate::Append { parent_id, component };
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
                    error: if found {
                        None
                    } else {
                        Some("Canvas not found".to_string())
                    },
                    data: Some(serde_json::json!({
                        "session_id": session_id,
                        "active": found,
                        "total_sessions": sessions.len(),
                    })),
                    execution_time: start.elapsed(),
                })
            }
            CanvasAction::Notify { session_id, level, message } => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canvas_component_arg_into_component() {
        let arg = CanvasComponentArg::Text {
            id: "t1".to_string(),
            content: "Hello".to_string(),
        };
        let comp = arg.into_component();
        assert!(
            matches!(comp, CanvasComponent::Text { id, content, .. } if id == "t1" && content == "Hello")
        );
    }

    #[test]
    fn test_canvas_component_arg_container() {
        let arg = CanvasComponentArg::Container {
            id: "root".to_string(),
            children: vec![CanvasComponentArg::Text {
                id: "c1".to_string(),
                content: "A".to_string(),
            }],
            layout: Some("vertical".to_string()),
        };
        let comp = arg.into_component();
        assert!(matches!(comp, CanvasComponent::Container { id, .. } if id == "root"));
    }

    #[test]
    fn test_canvas_component_arg_progress() {
        let arg = CanvasComponentArg::Progress {
            id: "p1".to_string(),
            value: 50.0,
            max: Some(100.0),
            label: Some("Progress".to_string()),
        };
        let comp = arg.into_component();
        assert!(
            matches!(comp, CanvasComponent::Progress { id, value, max, .. } if id == "p1" && value == 50.0 && max == Some(100.0))
        );
    }

    #[test]
    fn test_canvas_component_arg_table() {
        let arg = CanvasComponentArg::Table {
            id: "tbl1".to_string(),
            headers: vec!["A".to_string(), "B".to_string()],
            rows: vec![vec!["1".to_string(), "2".to_string()]],
        };
        let comp = arg.into_component();
        assert!(
            matches!(comp, CanvasComponent::Table { id, headers, rows } if id == "tbl1" && headers.len() == 2 && rows.len() == 1)
        );
    }

    #[test]
    fn test_canvas_component_arg_alert() {
        let arg = CanvasComponentArg::Alert {
            id: "a1".to_string(),
            level: "error".to_string(),
            message: "Oops".to_string(),
        };
        let comp = arg.into_component();
        assert!(
            matches!(comp, CanvasComponent::Alert { id, level, message } if id == "a1" && level == "error" && message == "Oops")
        );
    }

    #[test]
    fn test_canvas_tool_name_and_schema() {
        let manager = Arc::new(CanvasManager::new());
        let tool = CanvasTool::new(manager);
        assert_eq!(tool.name(), "canvas");
        let schema = tool.parameters_schema();
        assert!(schema.get("properties").is_some());
    }

    #[tokio::test]
    async fn test_canvas_present_images_inline() {
        let manager = Arc::new(CanvasManager::new());
        let tool = CanvasTool::new(manager.clone());
        let ctx = ToolContext::new("user", "conv");

        let result = tool
            .execute(
                serde_json::json!({
                    "action": "present",
                    "session_id": "sess-img",
                    "title": "Generated Image",
                    "components": [
                        {
                            "type": "image",
                            "id": "img1",
                            "src": "https://example.com/cat.png"
                        }
                    ]
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result
            .output
            .contains("![img1](https://example.com/cat.png)"));
        assert!(result.output.contains("**Generated Image**"));
        assert!(result
            .data
            .as_ref()
            .unwrap()
            .get("inline")
            .unwrap()
            .as_bool()
            .unwrap());

        // Should NOT create a canvas session when images are inline
        let sessions = manager.list_sessions().await;
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn test_canvas_present_no_images_goes_to_manager() {
        let manager = Arc::new(CanvasManager::new());
        let tool = CanvasTool::new(manager.clone());
        let ctx = ToolContext::new("user", "conv");

        let result = tool
            .execute(
                serde_json::json!({
                    "action": "present",
                    "session_id": "sess-text",
                    "components": [
                        {
                            "type": "text",
                            "id": "t1",
                            "content": "Hello"
                        }
                    ]
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.output.contains("Canvas presented"));

        // Should create a canvas session for non-image content
        let sessions = manager.list_sessions().await;
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn test_canvas_action_parsing() {
        let action: CanvasAction = serde_json::from_value(serde_json::json!({
            "action": "present",
            "session_id": "sess-1",
            "title": "My UI"
        }))
        .unwrap();
        assert!(
            matches!(action, CanvasAction::Present { session_id, .. } if session_id == "sess-1")
        );

        let action: CanvasAction = serde_json::from_value(serde_json::json!({
            "action": "hide",
            "session_id": "sess-1"
        }))
        .unwrap();
        assert!(matches!(action, CanvasAction::Hide { session_id } if session_id == "sess-1"));

        let action: CanvasAction = serde_json::from_value(serde_json::json!({
            "action": "snapshot",
            "session_id": "sess-1"
        }))
        .unwrap();
        assert!(matches!(action, CanvasAction::Snapshot { session_id } if session_id == "sess-1"));

        let action: CanvasAction = serde_json::from_value(serde_json::json!({
            "action": "notify",
            "session_id": "sess-1",
            "level": "info",
            "message": "Hello"
        }))
        .unwrap();
        assert!(
            matches!(action, CanvasAction::Notify { session_id, level, message } if session_id == "sess-1" && level == "info" && message == "Hello")
        );
    }

    #[test]
    fn test_canvas_component_arg_code() {
        let arg = CanvasComponentArg::Code {
            id: "code1".to_string(),
            content: "fn main() {}".to_string(),
            language: Some("rust".to_string()),
        };
        let comp = arg.into_component();
        assert!(
            matches!(comp, CanvasComponent::Code { id, content, language: Some(lang) } if id == "code1" && content == "fn main() {}" && lang == "rust")
        );
    }
}
