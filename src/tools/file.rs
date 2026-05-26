//! File operation tools for Manta
//!
//! Tools for reading, writing, and editing files.

use super::{create_schema, Tool, ToolContext, ToolExecutionResult};
use async_trait::async_trait;
use serde_json::Value;
#[cfg(test)]
use std::path::PathBuf;
use tokio::fs as tokio_fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

/// Maximum file size to read (1MB)
const MAX_FILE_SIZE: u64 = 1024 * 1024;

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
        let path_str = args["path"].as_str().ok_or_else(|| {
            crate::error::MantaError::Validation("Missing 'path' argument".to_string())
        })?;

        let path = context.resolve_path(std::path::Path::new(path_str));

        // Validate path is within allowed directories / workspace
        if !context.is_path_allowed(&path) {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' is outside the workspace or not in the allowlist",
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
            return Ok(ToolExecutionResult::error(format!("'{}' is not a file", path.display())));
        }

        // Check file size
        let metadata = tokio_fs::metadata(&path)
            .await
            .map_err(crate::error::MantaError::Io)?;

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
        let data = tokio_fs::read(&path)
            .await
            .map_err(crate::error::MantaError::Io)?;

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
        let path_str = args["path"].as_str().ok_or_else(|| {
            crate::error::MantaError::Validation("Missing 'path' argument".to_string())
        })?;

        let content = args["content"].as_str().ok_or_else(|| {
            crate::error::MantaError::Validation("Missing 'content' argument".to_string())
        })?;

        let path = context.resolve_path(std::path::Path::new(path_str));

        // Validate path is within allowed directories / workspace
        if !context.is_path_allowed(&path) {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' is outside the workspace or not in the allowlist",
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
            tokio_fs::create_dir_all(parent)
                .await
                .map_err(crate::error::MantaError::Io)?;
        }

        // Write file
        let mut file = tokio_fs::File::create(&path)
            .await
            .map_err(crate::error::MantaError::Io)?;

        file.write_all(content.as_bytes())
            .await
            .map_err(crate::error::MantaError::Io)?;

        file.flush().await.map_err(crate::error::MantaError::Io)?;

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
        let path_str = args["path"].as_str().ok_or_else(|| {
            crate::error::MantaError::Validation("Missing 'path' argument".to_string())
        })?;

        let old_string = args["old_string"].as_str().ok_or_else(|| {
            crate::error::MantaError::Validation("Missing 'old_string' argument".to_string())
        })?;

        let new_string = args["new_string"].as_str().ok_or_else(|| {
            crate::error::MantaError::Validation("Missing 'new_string' argument".to_string())
        })?;

        let path = context.resolve_path(std::path::Path::new(path_str));

        // Validate path is within allowed directories / workspace
        if !context.is_path_allowed(&path) {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' is outside the workspace or not in the allowlist",
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
        let content = tokio_fs::read_to_string(&path)
            .await
            .map_err(crate::error::MantaError::Io)?;

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
        tokio_fs::write(&path, new_content)
            .await
            .map_err(crate::error::MantaError::Io)?;

        info!("Made {} replacement(s) in {}", replacements, path.display());

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
        let pattern = args["pattern"].as_str().ok_or_else(|| {
            crate::error::MantaError::Validation("Missing 'pattern' argument".to_string())
        })?;

        let base_path = args["path"]
            .as_str()
            .map(|p| context.resolve_path(std::path::Path::new(p)))
            .unwrap_or_else(|| context.workspace_root.clone());

        if !context.is_path_allowed(&base_path) {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' is outside the workspace or not in the allowlist",
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
                return Ok(ToolExecutionResult::error(format!("Invalid glob pattern: {}", e)))
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
    use std::path::Path;

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
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);

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

    #[test]
    fn test_resolve_path_tilde() {
        let ctx = ToolContext::new("u", "c");
        let home = dirs::home_dir().unwrap();
        assert_eq!(ctx.resolve_path(Path::new("~/test")), home.join("test"));
        assert_eq!(ctx.resolve_path(Path::new("~")), home);
    }

    #[test]
    fn test_resolve_path_relative_to_workspace() {
        let ctx = ToolContext::new("u", "c").with_workspace_root("/tmp/workspace");
        assert_eq!(
            ctx.resolve_path(Path::new("src/main.rs")),
            PathBuf::from("/tmp/workspace/src/main.rs")
        );
    }

    #[test]
    fn test_resolve_path_absolute() {
        let ctx = ToolContext::new("u", "c").with_workspace_root("/tmp/workspace");
        assert_eq!(ctx.resolve_path(Path::new("/etc/passwd")), PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn test_truncate_content_no_truncate() {
        let content = "short".to_string();
        let result = FileReadTool::truncate_content(content.clone(), 100);
        assert_eq!(result, "short");
    }

    #[tokio::test]
    async fn test_file_read_missing_path() {
        let tool = FileReadTool::new();
        let context = ToolContext::new("user", "conv1");
        let args = serde_json::json!({});
        let result = tool.execute(args, &context).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_read_path_not_allowed() {
        let tool = FileReadTool::new();
        let context = ToolContext::new("user", "conv1").allow_path("/tmp/allowed");
        let args = serde_json::json!({"path": "/etc/passwd"});
        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("not in the allowlist"));
    }

    #[tokio::test]
    async fn test_file_read_not_found() {
        let tool = FileReadTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_only(false);
        let args = serde_json::json!({"path": "/tmp/nonexistent_file_12345.txt"});
        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_file_read_is_directory() {
        let temp_dir = std::env::temp_dir();
        let tool = FileReadTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);
        let args = serde_json::json!({"path": temp_dir.to_string_lossy()});
        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("is not a file"));
    }

    #[tokio::test]
    async fn test_file_read_binary() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("manta_bin_{}.bin", uuid::Uuid::new_v4()));

        tokio_fs::write(&test_file, b"Hello\x00World")
            .await
            .unwrap();

        let tool = FileReadTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);
        let args = serde_json::json!({"path": test_file.to_string_lossy()});
        let result = tool.execute(args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Binary file"));

        let _ = tokio_fs::remove_file(&test_file).await;
    }

    #[tokio::test]
    async fn test_file_read_with_limit() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("manta_limit_{}.txt", uuid::Uuid::new_v4()));

        tokio_fs::write(&test_file, "abcdefghij").await.unwrap();

        let tool = FileReadTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);
        let args = serde_json::json!({
            "path": test_file.to_string_lossy(),
            "limit": 5
        });
        let result = tool.execute(args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("abcde"));
        assert!(result.output.contains("truncated"));

        let _ = tokio_fs::remove_file(&test_file).await;
    }

    #[tokio::test]
    async fn test_file_write_missing_path() {
        let tool = FileWriteTool::new();
        let context = ToolContext::new("user", "conv1");
        let args = serde_json::json!({"content": "hello"});
        let result = tool.execute(args, &context).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_write_missing_content() {
        let tool = FileWriteTool::new();
        let context = ToolContext::new("user", "conv1");
        let args = serde_json::json!({"path": "/tmp/test.txt"});
        let result = tool.execute(args, &context).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_write_path_not_allowed() {
        let tool = FileWriteTool::new();
        let context = ToolContext::new("user", "conv1").allow_path("/tmp/allowed");
        let args = serde_json::json!({
            "path": "/etc/test_write.txt",
            "content": "hello"
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
    async fn test_file_write_without_backup() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("manta_nobak_{}.txt", uuid::Uuid::new_v4()));

        tokio_fs::write(&test_file, "original").await.unwrap();

        let tool = FileWriteTool::new().without_backup();
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);
        let args = serde_json::json!({
            "path": test_file.to_string_lossy(),
            "content": "updated"
        });
        let result = tool.execute(args, &context).await.unwrap();
        assert!(result.success);

        let backup_file = test_file.with_extension("bak");
        assert!(!backup_file.exists());

        let content = tokio_fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "updated");

        let _ = tokio_fs::remove_file(&test_file).await;
    }

    #[tokio::test]
    async fn test_file_write_creates_parent_dirs() {
        let temp_dir = std::env::temp_dir();
        let parent = temp_dir.join(format!("manta_parent_{}", uuid::Uuid::new_v4()));
        let test_file = parent.join("nested/file.txt");

        let tool = FileWriteTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);
        let args = serde_json::json!({
            "path": test_file.to_string_lossy(),
            "content": "nested content"
        });
        let result = tool.execute(args, &context).await.unwrap();
        assert!(result.success);
        assert!(test_file.exists());

        let _ = tokio_fs::remove_dir_all(&parent).await;
    }

    #[tokio::test]
    async fn test_file_edit_missing_args() {
        let tool = FileEditTool::new();
        let context = ToolContext::new("user", "conv1");

        // missing old_string
        let args = serde_json::json!({"path": "/tmp/test", "new_string": "x"});
        let result = tool.execute(args, &context).await;
        assert!(result.is_err());

        // missing new_string
        let args = serde_json::json!({"path": "/tmp/test", "old_string": "x"});
        let result = tool.execute(args, &context).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_edit_path_not_allowed() {
        let tool = FileEditTool::new();
        let context = ToolContext::new("user", "conv1").allow_path("/tmp/allowed");
        let args = serde_json::json!({
            "path": "/etc/passwd",
            "old_string": "root",
            "new_string": "admin"
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
    async fn test_file_edit_not_found() {
        let tool = FileEditTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_only(false);
        let args = serde_json::json!({
            "path": "/tmp/nonexistent_edit_12345.txt",
            "old_string": "old",
            "new_string": "new"
        });
        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_file_edit_string_not_found() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("manta_edit_{}.txt", uuid::Uuid::new_v4()));

        tokio_fs::write(&test_file, "hello world").await.unwrap();

        let tool = FileEditTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);
        let args = serde_json::json!({
            "path": test_file.to_string_lossy(),
            "old_string": "not present",
            "new_string": "replaced"
        });
        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("Could not find text"));

        let _ = tokio_fs::remove_file(&test_file).await;
    }

    #[tokio::test]
    async fn test_file_edit_success() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("manta_edit_ok_{}.txt", uuid::Uuid::new_v4()));

        tokio_fs::write(&test_file, "foo bar foo").await.unwrap();

        let tool = FileEditTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);
        let args = serde_json::json!({
            "path": test_file.to_string_lossy(),
            "old_string": "foo",
            "new_string": "baz"
        });
        let result = tool.execute(args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("2 replacement"));

        let content = tokio_fs::read_to_string(&test_file).await.unwrap();
        assert_eq!(content, "baz bar baz");

        let _ = tokio_fs::remove_file(&test_file).await;
    }

    #[tokio::test]
    async fn test_glob_missing_pattern() {
        let tool = GlobTool::new();
        let context = ToolContext::new("user", "conv1");
        let args = serde_json::json!({});
        let result = tool.execute(args, &context).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_glob_path_not_allowed() {
        let tool = GlobTool::new();
        let context = ToolContext::new("user", "conv1").allow_path("/tmp/allowed");
        let args = serde_json::json!({
            "pattern": "*.txt",
            "path": "/etc"
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
    async fn test_glob_invalid_pattern() {
        let tool = GlobTool::new();
        let context = ToolContext::new("user", "conv1");
        let args = serde_json::json!({"pattern": "[invalid"});
        let result = tool.execute(args, &context).await.unwrap();
        assert!(!result.success);
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("Invalid glob pattern"));
    }

    #[tokio::test]
    async fn test_glob_success() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("manta_glob_{}.txt", uuid::Uuid::new_v4()));

        tokio_fs::write(&test_file, "test").await.unwrap();

        let tool = GlobTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);
        let args = serde_json::json!({
            "pattern": "manta_glob_*.txt"
        });
        let result = tool.execute(args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains(test_file.to_string_lossy().as_ref()));

        let _ = tokio_fs::remove_file(&test_file).await;
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let temp_dir = std::env::temp_dir();
        let tool = GlobTool::new();
        let context = ToolContext::new("user", "conv1").with_workspace_root(&temp_dir);
        let args = serde_json::json!({
            "pattern": "no_such_file_*.xyz"
        });
        let result = tool.execute(args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("No files found"));
    }
}
