//! Linux Wayland screenshot tool using external capture utilities.

use crate::tools::{create_schema, Tool, ToolContext, ToolExecutionResult};
use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

/// Take screenshots on Linux Wayland using available capture tools.
///
/// Tries tools in order: `grim` → `spectacle` → `gnome-screenshot`.
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

    /// Find the first available screenshot utility.
    async fn find_capture_tool() -> Option<&'static str> {
        for cmd in &["grim", "spectacle", "gnome-screenshot"] {
            if Command::new("which")
                .arg(cmd)
                .output()
                .await
                .ok()
                .map_or(false, |o| o.status.success())
            {
                return Some(cmd);
            }
        }
        None
    }
}

#[async_trait]
impl Tool for ScreenshotTool {
    fn name(&self) -> &str {
        "linux_wayland_screenshot"
    }

    fn description(&self) -> &str {
        "Take a screenshot on Linux Wayland. Returns a base64-encoded PNG image. \
         Tries grim, spectacle, or gnome-screenshot."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Take a screenshot",
            serde_json::json!({
                "region": {
                    "type": "string",
                    "description": "Optional region to capture as x,y w h (e.g. \"100,100 500x300\") for grim"
                }
            }),
            Vec::<String>::new(),
        )
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let tool = match Self::find_capture_tool().await {
            Some(t) => t,
            None => {
                return Ok(ToolExecutionResult::error(
                    "No screenshot tool found. Install grim, spectacle, or gnome-screenshot."
                        .to_string(),
                ));
            }
        };

        let temp_path = std::env::temp_dir().join(format!(
            "syscity_screenshot_{}.png",
            uuid::Uuid::new_v4()
        ));

        info!(
            "Taking Wayland screenshot with {}: {}",
            tool,
            temp_path.display()
        );

        let region = args.get("region").and_then(|v| v.as_str());

        let result = match tool {
            "grim" => {
                let mut cmd = Command::new("grim");
                if let Some(reg) = region {
                    cmd.arg("-g").arg(reg);
                }
                cmd.arg(&temp_path).output().await
            }
            "spectacle" => {
                let mut cmd = Command::new("spectacle");
                cmd.arg("-f").arg("-n").arg("-o").arg(&temp_path);
                cmd.output().await
            }
            "gnome-screenshot" => {
                let mut cmd = Command::new("gnome-screenshot");
                cmd.arg("-f").arg(&temp_path);
                cmd.output().await
            }
            _ => unreachable!(),
        };

        let output = match timeout(Duration::from_secs(15), async { result }).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Ok(ToolExecutionResult::error(format!(
                    "Failed to run {}: {}",
                    tool, e
                )));
            }
            Err(_) => {
                return Ok(ToolExecutionResult::error(format!("{} timed out", tool)));
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("{} failed: {}", tool, stderr);
            return Ok(ToolExecutionResult::error(format!(
                "{} failed: {}",
                tool, stderr
            )));
        }

        let bytes = tokio::fs::read(&temp_path).await.map_err(|e| {
            crate::error::SyscityError::Storage {
                context: format!("Failed to read screenshot: {}", temp_path.display()),
                details: e.to_string(),
            }
        })?;

        let _ = tokio::fs::remove_file(&temp_path).await;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let data_url = format!("data:image/png;base64, {}", b64);

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

    fn is_available(&self, _context: &ToolContext) -> bool {
        std::env::var("WAYLAND_DISPLAY").is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screenshot_tool_creation() {
        let tool = ScreenshotTool::new();
        assert_eq!(tool.name(), "linux_wayland_screenshot");
    }
}
