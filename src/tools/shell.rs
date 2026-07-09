//! Shell tool for executing commands
//!
//! This tool allows the AI to execute shell commands in a sandboxed
//! environment.

use std::process::Stdio;

use async_trait::async_trait;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use super::{create_schema, Tool, ToolContext, ToolExecutionResult};
use crate::tools::sdk::ToolCapabilities;

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
            let mut end = self.max_output_size;
            while end > 0 && !output.is_char_boundary(end) {
                end -= 1;
            }
            let truncated = &output[..end];
            format!("{}\n[Output truncated: {} bytes total]", truncated, output.len())
        } else {
            output
        }
    }
}

/// Allowed shell interpreters. Any other value of `$SHELL` falls back to
/// `/bin/sh` to prevent a malicious or unexpected interpreter from being
/// invoked by the shell tool.
const ALLOWED_SHELLS: &[&str] = &[
    "/bin/sh",
    "/bin/bash",
    "/usr/bin/bash",
    "/bin/zsh",
    "/usr/bin/zsh",
];

/// Resolve the shell interpreter to use, validating against an allowlist.
fn resolve_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| ALLOWED_SHELLS.contains(&s.as_str()))
        .unwrap_or_else(|| "/bin/sh".to_string())
}

/// Returns `true` if `cmd` contains shell control operators that could be
/// used to chain additional commands beyond the first one.
///
/// Used to harden command-allowlist enforcement. Without this check a user
/// who allows `echo` could be hit by `echo hello && rm -rf /`.
fn contains_shell_control(cmd: &str) -> bool {
    // Strip the first "word" (the command name) — anything after is arguments
    // and shell operators.
    let rest = cmd
        .trim_start()
        .split_once(|c: char| c.is_whitespace())
        .map(|(_, rest)| rest)
        .unwrap_or("");
    if rest.is_empty() {
        return false;
    }

    // Check for shell control operators in the non-command portion.
    // We only check outside quotes to reduce false positives.
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            _ if in_single || in_double => continue,
            _ => {}
        }
        if in_single || in_double {
            continue;
        }
        match ch {
            ';' | '|' | '&' | '`' => return true,
            '$' if chars.peek() == Some(&'(') => return true,
            _ => {}
        }
    }
    false
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command for file operations, running scripts, or system commands. \
         Commands are executed with safety restrictions. Note: For scheduling or recurring tasks, \
         use the 'cron' tool instead — do NOT use shell commands with 'at', 'cron', or 'schedule'."
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
        let command_str = args["command"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("Missing 'command' argument".to_string())
        })?;

        // Check if command is allowed
        if !context.is_command_allowed(command_str) {
            return Ok(ToolExecutionResult::error(format!(
                "Command '{}' is not in the allowlist",
                command_str
            )));
        }

        // When an allowlist is active, reject shell control operators that
        // could chain additional commands beyond the allowed one.
        if !context.allowed_commands().is_empty() && contains_shell_control(command_str) {
            return Ok(ToolExecutionResult::error(format!(
                "Command '{}' contains shell control operators which are not permitted when \
                 command allowlisting is active",
                command_str
            )));
        }

        // Get working directory
        let working_dir = args["working_dir"]
            .as_str()
            .map(|p| context.resolve_path(std::path::Path::new(p)))
            .or_else(|| self.default_cwd.clone())
            .unwrap_or_else(|| context.workspace_root().clone());

        // Validate working directory
        if !context.is_path_allowed(&working_dir) {
            return Ok(ToolExecutionResult::error(format!(
                "Working directory '{}' is outside the workspace or not in the allowlist",
                working_dir.display()
            )));
        }

        info!("Executing shell command: {}", command_str);
        debug!("Working directory: {:?}", working_dir);

        // Parse command (handle shell operators like |, &&, etc.)
        let shell = resolve_shell();

        let start_time = std::time::Instant::now();

        // Build the command with resource limits if sandboxed
        let mut cmd = Command::new(&shell);
        cmd.arg("-c")
            .arg(command_str)
            .current_dir(&working_dir)
            .env_clear()
            .envs(context.environment())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Apply resource limits in sandboxed mode (Unix only)
        #[cfg(unix)]
        {
            if context.sandboxed() {
                let limits_summary = context.resource_limits_summary();
                debug!("Applying resource limits: {}", limits_summary);

                // Clone the limits to move into the closure
                let memory_limit = context.memory_limit();
                let cpu_limit = context.cpu_limit();
                let fd_limit = context.fd_limit();
                let process_limit = context.process_limit();

                // SAFETY: pre_exec runs in the child process after fork but before exec.
                // We only call async-signal-safe libc functions (setrlimit) here, which
                // is the documented safety requirement for pre_exec callbacks.
                #[allow(unsafe_code)]
                unsafe {
                    cmd.pre_exec(move || {
                        // Apply memory limit
                        if let Some(limit_bytes) = memory_limit {
                            let limit = libc::rlimit {
                                rlim_cur: limit_bytes as libc::rlim_t,
                                rlim_max: limit_bytes as libc::rlim_t,
                            };
                            if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                                return Err(std::io::Error::last_os_error());
                            }
                        }

                        // Apply CPU limit
                        if let Some(limit_secs) = cpu_limit {
                            let limit = libc::rlimit {
                                rlim_cur: limit_secs as libc::rlim_t,
                                rlim_max: limit_secs as libc::rlim_t,
                            };
                            if libc::setrlimit(libc::RLIMIT_CPU, &limit) != 0 {
                                return Err(std::io::Error::last_os_error());
                            }
                        }

                        // Apply FD limit
                        if let Some(limit_count) = fd_limit {
                            let limit = libc::rlimit {
                                rlim_cur: limit_count as libc::rlim_t,
                                rlim_max: limit_count as libc::rlim_t,
                            };
                            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                                return Err(std::io::Error::last_os_error());
                            }
                        }

                        // Apply process limit
                        if let Some(limit_count) = process_limit {
                            let limit = libc::rlimit {
                                rlim_cur: limit_count as libc::rlim_t,
                                rlim_max: limit_count as libc::rlim_t,
                            };
                            if libc::setrlimit(libc::RLIMIT_NPROC, &limit) != 0 {
                                return Err(std::io::Error::last_os_error());
                            }
                        }

                        Ok(())
                    });
                }
            }
        }

        let result = timeout(context.timeout(), cmd.output()).await;

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
                    Ok(ToolExecutionResult::success(truncated).with_execution_time(duration))
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
                Ok(ToolExecutionResult::error(format!("Execution failed: {}", e)))
            }
            Err(_) => {
                error!("Command timed out after {:?}", context.timeout());
                Ok(ToolExecutionResult::error(format!(
                    "Command timed out after {:?}",
                    context.timeout()
                )))
            }
        }
    }

    fn is_available(&self, context: &ToolContext) -> bool {
        // Shell is available if we're not in strict sandbox mode
        // or if there are allowed commands specified
        !context.sandboxed() || !context.allowed_commands().is_empty()
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: true,
            risk_level: crate::tools::approval::RiskLevel::High,
            categories: vec!["system".to_string(), "exec".to_string()],
            ..ToolCapabilities::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

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
        let tool = ShellTool::new();
        // Use a 1s timeout to avoid flakiness on oversubscribed CI runners.
        let context = ToolContext::new("user", "conv1").with_timeout(Duration::from_secs(1));

        let args = serde_json::json!({
            "command": "sleep 100"
        });

        let start = Instant::now();
        let result = tool.execute(args, &context).await.unwrap();
        let elapsed = start.elapsed();

        assert!(!result.success, "timed-out shell command should not report success");
        assert!(
            result.error.as_ref().unwrap().contains("timed out"),
            "expected timeout error, got {:?}",
            result.error
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "shell command was not killed within timeout: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_truncate_output_utf8_safe() {
        // Test that truncation does not panic on multi-byte UTF-8 characters.
        // The smiley emoji is 4 bytes. If max_output_size = 10 and the string
        // contains multi-byte chars near the boundary, the old code would panic.
        let tool = ShellTool::new().with_max_output_size(10);
        let output = "Hello 😀 world".to_string(); // 😀 is 4 bytes
        let truncated = tool.truncate_output(output.clone());
        assert!(truncated.contains("truncated"));
        // The prefix must be valid UTF-8 (no split multi-byte char)
        assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
    }

    #[test]
    fn test_truncate_output_exact_boundary() {
        let tool = ShellTool::new().with_max_output_size(4);
        let output = "😀😀😀".to_string(); // 12 bytes, each 😀 is 4 bytes
        let truncated = tool.truncate_output(output.clone());
        // With max 4 bytes, should keep exactly one emoji
        assert!(truncated.starts_with("😀"));
        assert!(truncated.contains("truncated"));
    }

    #[test]
    fn test_truncate_output_no_truncation_needed() {
        let tool = ShellTool::new().with_max_output_size(1000);
        let output = "Short".to_string();
        let truncated = tool.truncate_output(output.clone());
        assert_eq!(truncated, output);
    }

    #[tokio::test]
    async fn test_shell_tool_command_not_allowed() {
        let tool = ShellTool::new();
        let context = ToolContext::new("user", "conv1").allow_command("ls");

        let args = serde_json::json!({
            "command": "rm -rf /"
        });

        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("not in the allowlist"));
    }

    #[tokio::test]
    async fn test_shell_tool_command_injection_blocked() {
        let tool = ShellTool::new();
        let context = ToolContext::new("user", "conv1").allow_command("ls");

        // Command chaining should be blocked when only 'ls' is allowed
        let args = serde_json::json!({
            "command": "ls; rm -rf /"
        });

        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("not in the allowlist"));
    }

    #[tokio::test]
    async fn test_shell_tool_allowed_command_executes() {
        let tool = ShellTool::new();
        let context = ToolContext::new("user", "conv1").allow_command("echo");

        let args = serde_json::json!({
            "command": "echo hello"
        });

        let result = tool.execute(args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("hello"));
    }

    #[tokio::test]
    async fn test_shell_tool_and_operator_blocked_with_allowlist() {
        let tool = ShellTool::new();
        let context = ToolContext::new("user", "conv1").allow_command("echo");

        // `&&` should be blocked when allowlist is active, even though the
        // first token ("echo") is allowed — the operator chains additional
        // commands beyond the allowed one.
        let args = serde_json::json!({
            "command": "echo hello && rm -rf /"
        });

        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success, "chained && should be blocked");
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("shell control operators"));
    }

    #[tokio::test]
    async fn test_shell_tool_or_operator_blocked_with_allowlist() {
        let tool = ShellTool::new();
        let context = ToolContext::new("user", "conv1").allow_command("echo");

        let args = serde_json::json!({
            "command": "echo hello || rm -rf /"
        });

        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("shell control operators"));
    }

    #[test]
    fn test_contains_shell_control_semicolon() {
        assert!(contains_shell_control("echo a; echo b"));
    }

    #[test]
    fn test_contains_shell_control_and_operator() {
        assert!(contains_shell_control("echo a && echo b"));
    }

    #[test]
    fn test_contains_shell_control_pipe() {
        assert!(contains_shell_control("cat foo | grep bar"));
    }

    #[test]
    fn test_contains_shell_control_subshell() {
        assert!(contains_shell_control("echo $(whoami)"));
    }

    #[test]
    fn test_contains_shell_control_backtick() {
        assert!(contains_shell_control("echo `whoami`"));
    }

    #[test]
    fn test_contains_shell_control_simple_command() {
        assert!(!contains_shell_control("echo hello world"));
    }

    #[test]
    fn test_contains_shell_control_semicolon_in_quotes() {
        // A semicolon inside a quoted string is not a control operator.
        assert!(!contains_shell_control("echo 'hello; world'"));
    }

    #[test]
    fn test_contains_shell_control_no_args() {
        assert!(!contains_shell_control("ls"));
    }

    #[test]
    fn test_contains_shell_control_ampersand_in_url() {
        // `&` inside double-quotes should not trigger.
        assert!(!contains_shell_control("echo \"foo & bar\""));
    }
}
