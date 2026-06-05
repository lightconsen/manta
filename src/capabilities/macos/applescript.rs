//! AppleScript execution tool for macOS automation.

use crate::tools::{create_schema, Tool, ToolContext, ToolExecutionResult};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

/// Result of an AppleScript execution.
#[derive(Debug, Clone, Serialize)]
pub struct AppleScriptResult {
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Execute AppleScript on macOS using `osascript`.
#[derive(Debug)]
pub struct AppleScriptTool;

impl Default for AppleScriptTool {
    fn default() -> Self {
        Self::new()
    }
}

impl AppleScriptTool {
    pub fn new() -> Self {
        Self
    }

    /// Execute an AppleScript string and return the result.
    pub async fn execute_script(script: &str, timeout_secs: u64) -> AppleScriptResult {
        info!("Executing AppleScript ({} chars)", script.len());

        let script_owned = script.to_string();
        let result = timeout(
            Duration::from_secs(timeout_secs),
            async {
                let mut child = Command::new("osascript")
                    .arg("-")
                    .stdin(std::process::Stdio::piped())
                    .spawn()?;

                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(script_owned.as_bytes()).await;
                }

                child.wait_with_output().await
            },
        )
        .await;

        match result {
            Ok(Ok(output)) => {

                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                AppleScriptResult {
                    success: output.status.success(),
                    output: stdout,
                    error: if output.status.success() || stderr.is_empty() {
                        None
                    } else {
                        Some(stderr)
                    },
                }
            }
            Ok(Err(e)) => {
                warn!("Failed to run osascript: {}", e);
                AppleScriptResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to run osascript: {}", e)),
                }
            }
            Err(_) => {
                warn!("osascript timed out");
                AppleScriptResult {
                    success: false,
                    output: String::new(),
                    error: Some("osascript timed out".to_string()),
                }
            }
        }
    }
}

#[async_trait]
impl Tool for AppleScriptTool {
    fn name(&self) -> &str {
        "applescript"
    }

    fn description(&self) -> &str {
        "Execute AppleScript on macOS using `osascript`. \
         Use to automate macOS applications, query UI state, \
         control system settings, or interact with app-specific APIs."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Execute AppleScript",
            serde_json::json!({
                "script": {
                    "type": "string",
                    "description": "The AppleScript source code to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds",
                    "default": 30
                }
            }),
            vec!["script"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let script = args["script"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("Missing 'script' argument".to_string())
        })?;

        let timeout_secs = args["timeout"].as_u64().unwrap_or(30);

        let result = Self::execute_script(script, timeout_secs).await;

        let json = serde_json::to_string_pretty(&result)
            .map_err(crate::error::SyscityError::Serialization)?;

        if result.success {
            Ok(ToolExecutionResult::success(json).with_data(serde_json::to_value(result)?))
        } else {
            Ok(ToolExecutionResult::error(result.error.clone().unwrap_or_default()).with_data(serde_json::to_value(result)?))
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
    fn test_applescript_tool_creation() {
        let tool = AppleScriptTool::new();
        assert_eq!(tool.name(), "applescript");
        assert!(tool.description().contains("AppleScript"));
    }
}
