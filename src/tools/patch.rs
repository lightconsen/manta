//! Patch application tool
//!
//! Apply unified diff patches to files using git apply.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

use super::{Tool, ToolContext, ToolExecutionResult};

/// Apply a unified diff patch to files.
pub struct ApplyPatchTool;

impl Default for ApplyPatchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct ApplyPatchArgs {
    /// Unified diff patch content
    patch: String,
    /// Target directory (default: current working directory)
    #[serde(default)]
    directory: String,
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to files. The patch should be in standard unified diff format (as produced by git diff or diff -u)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch content"
                },
                "directory": {
                    "type": "string",
                    "description": "Target directory for patch application (default: current directory)",
                    "default": "."
                }
            },
            "required": ["patch"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: ApplyPatchArgs = match serde_json::from_value(args) {
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

        let target_dir = if args.directory.is_empty() {
            context.working_directory.clone()
        } else {
            std::path::PathBuf::from(&args.directory)
        };

        // Path sandboxing: validate that target_dir stays within working_directory
        let canonical_workdir = tokio::fs::canonicalize(&context.working_directory)
            .await
            .ok();
        let canonical_target = if target_dir.is_absolute() {
            tokio::fs::canonicalize(&target_dir).await.ok()
        } else {
            tokio::fs::canonicalize(context.working_directory.join(&target_dir))
                .await
                .ok()
        };
        if let (Some(ref workdir), Some(ref target)) = (&canonical_workdir, &canonical_target) {
            if !target.starts_with(workdir) {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Patch target directory is outside the working directory: {}",
                        target.display()
                    )),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        }

        let patch_file = target_dir.join(format!("syscity_patch_{}.diff", uuid::Uuid::new_v4()));
        match tokio::fs::write(&patch_file, &args.patch).await {
            Ok(_) => {}
            Err(e) => {
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to write patch file: {}", e)),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        }

        let result = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new("git")
                .args(["apply", "--check", patch_file.to_str().unwrap_or("")])
                .current_dir(&target_dir)
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) if output.status.success() => {}
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stderr_msg = if stderr.is_empty() {
                    String::from(
                        "Patch does not apply cleanly. Check the patch format and target files.",
                    )
                } else {
                    stderr.into_owned()
                };
                let _ = tokio::fs::remove_file(&patch_file).await;
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(stderr_msg),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
            Ok(Err(e)) => {
                let _ = tokio::fs::remove_file(&patch_file).await;
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Patch execution failed: {}", e)),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
            Err(_) => {
                let _ = tokio::fs::remove_file(&patch_file).await;
                return Ok(ToolExecutionResult {
                    success: false,
                    output: String::new(),
                    error: Some("Patch check timed out.".to_string()),
                    data: None,
                    execution_time: start.elapsed(),
                });
            }
        }

        let apply_result = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new("git")
                .args(["apply", patch_file.to_str().unwrap_or("")])
                .current_dir(&target_dir)
                .output(),
        )
        .await;

        let _ = tokio::fs::remove_file(&patch_file).await;

        match apply_result {
            Ok(Ok(output)) if output.status.success() => Ok(ToolExecutionResult {
                success: true,
                output: "Patch applied successfully".to_string(),
                error: None,
                data: None,
                execution_time: start.elapsed(),
            }),
            Ok(Ok(output)) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Patch application failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )),
                data: None,
                execution_time: start.elapsed(),
            }),
            Ok(Err(e)) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to run git apply: {}", e)),
                data: None,
                execution_time: start.elapsed(),
            }),
            Err(_) => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some("Patch application timed out".to_string()),
                data: None,
                execution_time: start.elapsed(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_patch_args_parsing() {
        let args: ApplyPatchArgs = serde_json::from_value(serde_json::json!({
            "patch": "diff content",
            "directory": "/tmp"
        }))
        .unwrap();
        assert_eq!(args.patch, "diff content");
        assert_eq!(args.directory, "/tmp");

        let args2: ApplyPatchArgs = serde_json::from_value(serde_json::json!({
            "patch": "diff content"
        }))
        .unwrap();
        assert_eq!(args2.directory, "");
    }
}
