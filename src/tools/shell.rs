//! Shell tool for executing commands
//!
//! This tool allows the AI to execute shell commands in a sandboxed environment.

use super::{Tool, ToolContext, ToolExecutionResult, create_schema};
use async_trait::async_trait;
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

/// Shell tool for executing commands
#[derive(Debug)]
pub struct ShellTool {
    /// Default working directory
    default_cwd: Option<std::path::PathBuf>,
    /// Maximum output size in bytes
    max_output_size: usize,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self {
            default_cwd: None,
            max_output_size: 10 * 1024, // 10 KB
        }
    }
}

impl ShellTool {
    /// Create a new shell tool
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the default working directory
    pub fn with_default_cwd(mut self, cwd: impl Into<std::path::PathBuf>) -> Self {
        self.default_cwd = Some(cwd.into());
        self
    }

    /// Set the maximum output size
    pub fn with_max_output_size(mut self, size: usize) -> Self {
        self.max_output_size = size;
        self
    }

    /// Truncate output if it exceeds the limit
    fn truncate_output(&self, output: String) -> String {
        if output.len() > self.max_output_size {
            let truncated = &output[..self.max_output_size];
            format!("{}\n[Output truncated: {} bytes total]", truncated, output.len())
        } else {
            output
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command. Use for file operations, running scripts, or system commands. \
         Commands are executed with safety restrictions."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Execute a shell command",
            serde_json::json!({
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional working directory for the command"
                }
            }),
            vec!["command"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let command_str = args["command"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'command' argument".to_string()))?;

        // Check if command is allowed
        if !context.is_command_allowed(command_str) {
            return Ok(ToolExecutionResult::error(format!(
                "Command '{}' is not in the allowlist",
                command_str
            )));
        }

        // Get working directory
        let working_dir = args["working_dir"]
            .as_str()
            .map(std::path::PathBuf::from)
            .or_else(|| self.default_cwd.clone())
            .unwrap_or_else(|| context.working_directory.clone());

        // Validate working directory
        if !context.is_path_allowed(&working_dir) {
            return Ok(ToolExecutionResult::error(format!(
                "Working directory '{}' is not in the allowlist",
                working_dir.display()
            )));
        }

        info!("Executing shell command: {}", command_str);
        debug!("Working directory: {:?}", working_dir);

        // Parse command (handle shell operators like |, &&, etc.)
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

        let start_time = std::time::Instant::now();

        let result = timeout(
            context.timeout,
            Command::new(&shell)
                .arg("-c")
                .arg(command_str)
                .current_dir(&working_dir)
                .env_clear()
                .envs(&context.environment)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await;

        let duration = start_time.elapsed();

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                let combined_output = if stderr.is_empty() {
                    stdout
                } else {
                    format!("{}{}", stdout, stderr)
                };

                let truncated = self.truncate_output(combined_output);

                if output.status.success() {
                    info!("Command executed successfully in {:?}", duration);
                    Ok(ToolExecutionResult::success(truncated)
                        .with_execution_time(duration))
                } else {
                    let exit_code = output.status.code().unwrap_or(-1);
                    warn!("Command failed with exit code {}: {}", exit_code, command_str);
                    Ok(ToolExecutionResult::error(format!(
                        "Exit code {}: {}",
                        exit_code, truncated
                    ))
                    .with_execution_time(duration))
                }
            }
            Ok(Err(e)) => {
                error!("Failed to execute command: {}", e);
                Ok(ToolExecutionResult::error(format!(
                    "Execution failed: {}",
                    e
                )))
            }
            Err(_) => {
                error!("Command timed out after {:?}", context.timeout);
                Ok(ToolExecutionResult::error(format!(
                    "Command timed out after {:?}",
                    context.timeout
                )))
            }
        }
    }

    fn is_available(&self, context: &ToolContext) -> bool {
        // Shell is available if we're not in strict sandbox mode
        // or if there are allowed commands specified
        !context.sandboxed || !context.allowed_commands.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_tool_creation() {
        let tool = ShellTool::new();
        assert_eq!(tool.name(), "shell");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_truncate_output() {
        let tool = ShellTool::new().with_max_output_size(10);
        let output = "This is a very long string that definitely exceeds the limit".to_string();
        let truncated = tool.truncate_output(output.clone());
        // Truncated output contains the truncation message
        assert!(truncated.contains("truncated"));
        // The output was actually truncated (contains the prefix of the original)
        assert!(truncated.starts_with("This is a "));
    }

    #[tokio::test]
    async fn test_shell_tool_execute() {
        let tool = ShellTool::new();
        let context = ToolContext::new("user", "conv1");

        let args = serde_json::json!({
            "command": "echo hello"
        });

        let result = tool.execute(args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_shell_tool_timeout() {
        // Skip this test on macOS as sleep behaves differently
        if cfg!(target_os = "macos") {
            return;
        }

        let tool = ShellTool::new();
        let context = ToolContext::new("user", "conv1")
            .with_timeout(Duration::from_millis(100));

        let args = serde_json::json!({
            "command": "sleep 5"
        });

        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result.output.to_lowercase().contains("timed out"));
    }
}
