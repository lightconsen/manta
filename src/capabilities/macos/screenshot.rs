//! macOS screenshot tool using `screencapture`.

use crate::tools::{create_schema, Tool, ToolContext, ToolExecutionResult};
use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

/// Take screenshots on macOS using the built-in `screencapture` utility.
#[derive(Debug)]
pub struct ScreenshotTool;

impl Default for ScreenshotTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenshotTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "macos_screenshot"
    }

    fn description(&self) -> &str {
        "Take a screenshot on macOS using `screencapture`. \
         Returns a base64-encoded PNG image. \
         Use when you need to visually verify the current screen state."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Take a screenshot",
            serde_json::json!({
                "display": {
                    "type": "integer",
                    "description": "Display number to capture (default: main display)",
                    "default": 1
                },
                "window": {
                    "type": "string",
                    "description": "Optional window name to capture instead of full screen"
                }
            }),
            Vec::<String>::new(),
        )
    }

    async fn execute(
        &self,
        _args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let temp_path = std::env::temp_dir().join(format!(
            "syscity_screenshot_{}.png",
            uuid::Uuid::new_v4()
        ));

        info!("Taking screenshot: {}", temp_path.display());

        let result = timeout(
            Duration::from_secs(15),
            Command::new("screencapture")
                .arg("-x") // no camera shutter sound
                .arg("-C") // capture cursor
                .arg(&temp_path)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) if output.status.success() => {
                let bytes = tokio::fs::read(&temp_path).await.map_err(|e| {
                    crate::error::SyscityError::Storage {
                        context: format!("Failed to read screenshot: {}", temp_path.display()),
                        details: e.to_string(),
                    }
                })?;

                // Clean up temp file
                let _ = tokio::fs::remove_file(&temp_path).await;

                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let data_url = format!("data:image/png;base64,{}", b64);

                Ok(ToolExecutionResult::success(format!(
                    "Screenshot captured ({} bytes, base64: {}...)",
                    bytes.len(),
                    &data_url[..data_url.len().min(80)]
                ))
                .with_data(serde_json::json!({
                    "image_base64": b64,
                    "data_url": data_url,
                    "format": "png",
                    "size": bytes.len()
                })))
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!("screencapture failed: {}", stderr);
                Ok(ToolExecutionResult::error(format!(
                    "screencapture failed: {}",
                    stderr
                )))
            }
            Ok(Err(e)) => Ok(ToolExecutionResult::error(format!(
                "Failed to run screencapture: {}",
                e
            ))),
            Err(_) => Ok(ToolExecutionResult::error(
                "screencapture timed out".to_string()
            )),
        }
    }

    fn is_available(&self, _context: &ToolContext) -> bool {
        cfg!(target_os = "macos")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screenshot_tool_creation() {
        let tool = ScreenshotTool::new();
        assert_eq!(tool.name(), "macos_screenshot");
        assert!(tool.description().contains("screenshot"));
    }
}
