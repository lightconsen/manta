//! File operation tools for Manta
//!
//! Tools for reading, writing, and editing files.

use super::{Tool, ToolContext, ToolExecutionResult, create_schema};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use tokio::fs as tokio_fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

/// Maximum file size to read (1MB)
const MAX_FILE_SIZE: u64 = 1024 * 1024;

/// Expand tilde (~) to home directory
fn expand_home(path: &str) -> PathBuf {
    if path.starts_with("~/") || path == "~" {
        if let Some(home) = dirs::home_dir() {
            let rest = &path[1..]; // Remove the leading ~
            return home.join(rest.trim_start_matches('/'));
        }
    }
    PathBuf::from(path)
}

/// File read tool
#[derive(Debug, Default)]
pub struct FileReadTool;

impl FileReadTool {
    /// Create a new file read tool
    pub fn new() -> Self {
        Self
    }

    /// Check if file is binary
    fn is_binary(data: &[u8]) -> bool {
        // Simple heuristic: check for null bytes in first 1KB
        let check_len = data.len().min(1024);
        data[..check_len].contains(&0)
    }

    /// Truncate file content if too large
    fn truncate_content(content: String, max_chars: usize) -> String {
        if content.len() > max_chars {
            format!(
                "{}\n[File truncated: {} total characters]",
                &content[..max_chars],
                content.len()
            )
        } else {
            content
        }
    }
}

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Can read text files and detect binary files. \
         Maximum file size: 1MB."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Read a file's contents",
            serde_json::json!({
                "path": {
                    "type": "string",
                    "description": "The path to the file to read"
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional maximum number of characters to read"
                }
            }),
            vec!["path"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'path' argument".to_string()))?;

        let path = expand_home(path_str);

        // Validate path is within allowed directories
        if !context.is_path_allowed(&path) {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' is not in the allowlist",
                path.display()
            )));
        }

        // Check file exists
        if !path.exists() {
            return Ok(ToolExecutionResult::error(format!(
                "File '{}' does not exist",
                path.display()
            )));
        }

        // Check it's a file, not a directory
        if !path.is_file() {
            return Ok(ToolExecutionResult::error(format!(
                "'{}' is not a file",
                path.display()
            )));
        }

        // Check file size
        let metadata = tokio_fs::metadata(&path).await.map_err(|e| {
            crate::error::MantaError::Io(e)
        })?;

        if metadata.len() > MAX_FILE_SIZE {
            return Ok(ToolExecutionResult::error(format!(
                "File '{}' is too large ({} bytes, max {})",
                path.display(),
                metadata.len(),
                MAX_FILE_SIZE
            )));
        }

        info!("Reading file: {}", path.display());

        // Read file
        let data = tokio_fs::read(&path).await.map_err(|e| {
            crate::error::MantaError::Io(e)
        })?;

        // Check if binary
        if Self::is_binary(&data) {
            return Ok(ToolExecutionResult::success(format!(
                "[Binary file: {} bytes]",
                data.len()
            )));
        }

        // Convert to string
        let content = String::from_utf8_lossy(&data).to_string();

        // Apply limit if specified
        let limit = args["limit"].as_u64().map(|l| l as usize);
        let final_content = if let Some(lim) = limit {
            Self::truncate_content(content, lim)
        } else {
            content
        };

        Ok(ToolExecutionResult::success(final_content))
    }
}

/// File write tool
#[derive(Debug)]
pub struct FileWriteTool {
    /// Whether to backup existing files
    backup: bool,
}

impl Default for FileWriteTool {
    fn default() -> Self {
        Self { backup: true }
    }
}

impl FileWriteTool {
    /// Create a new file write tool
    pub fn new() -> Self {
        Self::default()
    }

    /// Disable backup of existing files
    pub fn without_backup(mut self) -> Self {
        self.backup = false;
        self
    }
}

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates parent directories if needed. \
         Optionally backs up existing files."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Write content to a file",
            serde_json::json!({
                "path": {
                    "type": "string",
                    "description": "The path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            }),
            vec!["path", "content"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'path' argument".to_string()))?;

        let content = args["content"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'content' argument".to_string()))?;

        let path = expand_home(path_str);

        // Validate path is within allowed directories
        if !context.is_path_allowed(&path) {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' is not in the allowlist",
                path.display()
            )));
        }

        // Backup existing file if requested
        if self.backup && path.exists() {
            let backup_path = path.with_extension("bak");
            if let Err(e) = tokio_fs::copy(&path, &backup_path).await {
                warn!("Failed to create backup: {}", e);
            } else {
                debug!("Created backup: {}", backup_path.display());
            }
        }

        // Create parent directories
        if let Some(parent) = path.parent() {
            tokio_fs::create_dir_all(parent).await.map_err(|e| {
                crate::error::MantaError::Io(e)
            })?;
        }

        // Write file
        let mut file = tokio_fs::File::create(&path).await.map_err(|e| {
            crate::error::MantaError::Io(e)
        })?;

        file.write_all(content.as_bytes()).await.map_err(|e| {
            crate::error::MantaError::Io(e)
        })?;

        info!("Wrote {} bytes to {}", content.len(), path.display());

        Ok(ToolExecutionResult::success(format!(
            "Successfully wrote {} bytes to '{}'",
            content.len(),
            path.display()
        )))
    }
}

/// File edit tool (find and replace)
#[derive(Debug, Default)]
pub struct FileEditTool;

impl FileEditTool {
    /// Create a new file edit tool
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for FileEditTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing text. Supports finding and replacing strings."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Edit a file by replacing text",
            serde_json::json!({
                "path": {
                    "type": "string",
                    "description": "The path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to find and replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text"
                }
            }),
            vec!["path", "old_string", "new_string"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'path' argument".to_string()))?;

        let old_string = args["old_string"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'old_string' argument".to_string()))?;

        let new_string = args["new_string"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'new_string' argument".to_string()))?;

        let path = expand_home(path_str);

        // Validate path
        if !context.is_path_allowed(&path) {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' is not in the allowlist",
                path.display()
            )));
        }

        // Check file exists
        if !path.exists() {
            return Ok(ToolExecutionResult::error(format!(
                "File '{}' does not exist",
                path.display()
            )));
        }

        // Read file
        let content = tokio_fs::read_to_string(&path).await.map_err(|e| {
            crate::error::MantaError::Io(e)
        })?;

        // Check if old_string exists
        if !content.contains(old_string) {
            return Ok(ToolExecutionResult::error(format!(
                "Could not find text to replace in '{}'",
                path.display()
            )));
        }

        // Replace
        let new_content = content.replace(old_string, new_string);
        let replacements = content.matches(old_string).count();

        // Write back
        tokio_fs::write(&path, new_content).await.map_err(|e| {
            crate::error::MantaError::Io(e)
        })?;

        info!(
            "Made {} replacement(s) in {}",
            replacements,
            path.display()
        );

        Ok(ToolExecutionResult::success(format!(
            "Successfully made {} replacement(s) in '{}'",
            replacements,
            path.display()
        )))
    }
}

/// Glob tool for finding files
#[derive(Debug, Default)]
pub struct GlobTool;

impl GlobTool {
    /// Create a new glob tool
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern. Returns up to 100 matching files."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Find files matching a pattern",
            serde_json::json!({
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match (e.g., '*.rs', 'src/**/*.txt')"
                },
                "path": {
                    "type": "string",
                    "description": "Optional directory to search in (defaults to current directory)"
                }
            }),
            vec!["pattern"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'pattern' argument".to_string()))?;

        let base_path = args["path"]
            .as_str()
            .map(expand_home)
            .unwrap_or_else(|| context.working_directory.clone());

        if !context.is_path_allowed(&base_path) {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' is not in the allowlist",
                base_path.display()
            )));
        }

        // Use glob crate to find files
        let pattern_full = base_path.join(pattern);
        let pattern_str = pattern_full.to_string_lossy();

        let mut files = Vec::new();
        match glob::glob(&pattern_str) {
            Ok(entries) => {
                for entry in entries.take(100) {
                    match entry {
                        Ok(path) => {
                            if path.is_file() {
                                files.push(path.to_string_lossy().to_string());
                            }
                        }
                        Err(e) => warn!("Error reading glob entry: {}", e),
                    }
                }
            }
            Err(e) => {
                return Ok(ToolExecutionResult::error(format!(
                    "Invalid glob pattern: {}",
                    e
                )))
            }
        }

        let output = if files.is_empty() {
            "No files found matching the pattern".to_string()
        } else {
            files.join("\n")
        };

        Ok(ToolExecutionResult::success(output)
            .with_data(serde_json::json!({ "count": files.len() })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_read_tool() {
        let tool = FileReadTool::new();
        assert_eq!(tool.name(), "file_read");
    }

    #[test]
    fn test_is_binary() {
        let binary = b"Hello\x00World";
        assert!(FileReadTool::is_binary(binary));

        let text = b"Hello World";
        assert!(!FileReadTool::is_binary(text));
    }

    #[test]
    fn test_truncate_content() {
        let content = "a".repeat(1000);
        let truncated = FileReadTool::truncate_content(content.clone(), 100);
        assert!(truncated.len() < content.len());
        assert!(truncated.contains("truncated"));
    }

    #[tokio::test]
    async fn test_file_write_and_read() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("manta_test_{}.txt", uuid::Uuid::new_v4()));

        // Write
        let write_tool = FileWriteTool::new();
        let context = ToolContext::new("user", "conv1")
            .with_working_dir(&temp_dir);

        let write_args = serde_json::json!({
            "path": test_file.to_string_lossy(),
            "content": "Hello, World!"
        });

        let result = write_tool.execute(write_args, &context).await.unwrap();
        assert!(result.success);

        // Read
        let read_tool = FileReadTool::new();
        let read_args = serde_json::json!({
            "path": test_file.to_string_lossy()
        });

        let result = read_tool.execute(read_args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Hello, World!"));

        // Cleanup
        let _ = tokio_fs::remove_file(&test_file).await;
    }
}
