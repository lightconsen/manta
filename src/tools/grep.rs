//! Grep tool for searching file contents
//!
//! This tool searches file contents using regex patterns, similar to the grep command.

use super::{Tool, ToolContext, ToolExecutionResult, create_schema};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info, warn};

/// Maximum file size to search (5MB)
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Maximum results to return
const MAX_RESULTS: usize = 100;

/// Maximum context lines
const MAX_CONTEXT_LINES: usize = 10;

/// Grep tool for searching file contents
#[derive(Debug, Default)]
pub struct GrepTool;

impl GrepTool {
    /// Create a new grep tool
    pub fn new() -> Self {
        Self
    }

    /// Search a single file
    async fn search_file(
        &self,
        pattern: &regex::Regex,
        path: &Path,
        context_lines: usize,
    ) -> crate::Result<Vec<SearchMatch>> {
        let content = fs::read_to_string(path).await.map_err(crate::error::MantaError::Io)?;

        let mut matches = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        for (line_num, line) in lines.iter().enumerate() {
            if pattern.is_match(line) {
                let context_before: Vec<String> = lines
                    .iter()
                    .take(line_num)
                    .rev()
                    .take(context_lines)
                    .rev()
                    .map(|&s| s.to_string())
                    .collect();

                let context_after: Vec<String> = lines
                    .iter()
                    .skip(line_num + 1)
                    .take(context_lines)
                    .map(|&s| s.to_string())
                    .collect();

                matches.push(SearchMatch {
                    file: path.to_string_lossy().to_string(),
                    line_number: line_num + 1,
                    line_content: line.to_string(),
                    context_before,
                    context_after,
                });
            }
        }

        Ok(matches)
    }

    /// Recursively search directory
    fn search_directory<'a>(
        &'a self,
        pattern: &'a regex::Regex,
        dir: &'a Path,
        include_pattern: Option<&'a str>,
        context_lines: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::Result<Vec<SearchMatch>>> + Send + 'a>> {
        Box::pin(async move {
        let mut all_matches = Vec::new();
        let mut entries = fs::read_dir(dir).await.map_err(crate::error::MantaError::Io)?;

        while let Some(entry) = entries.next_entry().await.map_err(crate::error::MantaError::Io)? {
            if all_matches.len() >= MAX_RESULTS {
                break;
            }

            let path = entry.path();

            if path.is_dir() {
                // Skip hidden directories and common non-source directories
                if let Some(name) = path.file_name() {
                    let name = name.to_string_lossy();
                    if name.starts_with('.') || name == "node_modules" || name == "target" || name == "__pycache__" {
                        continue;
                    }
                }

                // Recursively search subdirectories
                let sub_matches = self.search_directory(pattern, &path, include_pattern, context_lines).await?;
                all_matches.extend(sub_matches);
            } else if path.is_file() {
                // Check file size
                if let Ok(metadata) = fs::metadata(&path).await {
                    if metadata.len() > MAX_FILE_SIZE {
                        warn!("Skipping large file: {}", path.display());
                        continue;
                    }
                }

                // Check include pattern
                if let Some(inc) = include_pattern {
                    if !self.matches_pattern(&path, inc) {
                        continue;
                    }
                }

                // Check if file is text
                if let Ok(content) = fs::read(&path).await {
                    if self.is_binary(&content) {
                        continue;
                    }
                }

                // Search the file
                match self.search_file(pattern, &path, context_lines).await {
                    Ok(mut matches) => {
                        all_matches.append(&mut matches);
                    }
                    Err(e) => {
                        debug!("Error searching file {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(all_matches)
        })
    }

    /// Check if file matches an include pattern
    fn matches_pattern(&self, path: &Path, pattern: &str) -> bool {
        if let Some(filename) = path.file_name() {
            let filename = filename.to_string_lossy();
            // Simple glob matching
            if pattern.contains('*') {
                let parts: Vec<&str> = pattern.split('*').collect();
                if parts.len() == 2 {
                    return filename.starts_with(parts[0]) && filename.ends_with(parts[1]);
                }
            }
            return filename == pattern || filename.ends_with(pattern);
        }
        false
    }

    /// Check if content is binary
    fn is_binary(&self, content: &[u8]) -> bool {
        let check_len = content.len().min(1024);
        content[..check_len].contains(&0)
    }

    /// Format search results
    fn format_results(&self, matches: &[SearchMatch], output_format: &str) -> String {
        match output_format {
            "json" => {
                serde_json::to_string_pretty(matches).unwrap_or_else(|_| "[]".to_string())
            }
            "compact" => {
                matches
                    .iter()
                    .map(|m| format!("{}:{}:{}", m.file, m.line_number, m.line_content.trim()))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => {
                // Pretty format with context
                matches
                    .iter()
                    .map(|m| {
                        let mut output = format!("\n{}:{}\n", m.file, m.line_number);

                        // Context before
                        for (i, line) in m.context_before.iter().enumerate() {
                            let line_num = m.line_number - m.context_before.len() + i;
                            output.push_str(&format!("{:4}  {}\n", line_num, line));
                        }

                        // Match line
                        output.push_str(&format!("{:4}> {}\n", m.line_number, m.line_content));

                        // Context after
                        for (i, line) in m.context_after.iter().enumerate() {
                            let line_num = m.line_number + i + 1;
                            output.push_str(&format!("{:4}  {}\n", line_num, line));
                        }

                        output
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    }
}

/// A search match
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchMatch {
    file: String,
    line_number: usize,
    line_content: String,
    context_before: Vec<String>,
    context_after: Vec<String>,
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        r#"Search file contents using regex patterns. Similar to the grep command.

Use this to:
- Find text in files
- Search for patterns across multiple files
- Find where functions or variables are defined
- Search with context lines around matches

Supports regex patterns and can search recursively through directories."#
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Search file contents",
            serde_json::json!({
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search (defaults to current directory)"
                },
                "include": {
                    "type": "string",
                    "description": "File pattern to include, e.g., '*.rs', '*.py' (optional)"
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines to show before/after matches (0-10)",
                    "default": 2
                },
                "format": {
                    "type": "string",
                    "enum": ["pretty", "compact", "json"],
                    "description": "Output format",
                    "default": "pretty"
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
        let pattern_str = args["pattern"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'pattern' argument".to_string()))?;

        // Compile regex
        let pattern = regex::Regex::new(pattern_str)
            .map_err(|e| crate::error::MantaError::Validation(format!("Invalid regex pattern: {}", e)))?;

        let path_str = args["path"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| context.working_directory.clone());

        // Validate path
        if !context.is_path_allowed(&path_str) {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' is not in the allowlist",
                path_str.display()
            )));
        }

        let include_pattern = args["include"].as_str();
        let context_lines = args["context"]
            .as_u64()
            .map(|c| c as usize)
            .unwrap_or(2)
            .min(MAX_CONTEXT_LINES);
        let output_format = args["format"].as_str().unwrap_or("pretty");

        info!(
            "Searching for '{}' in '{}'",
            pattern_str,
            path_str.display()
        );

        // Perform search
        let matches = if path_str.is_file() {
            self.search_file(&pattern, &path_str, context_lines).await?
        } else if path_str.is_dir() {
            self.search_directory(&pattern, &path_str, include_pattern, context_lines).await?
        } else {
            return Ok(ToolExecutionResult::error(format!(
                "Path '{}' does not exist",
                path_str.display()
            )));
        };

        let count = matches.len();

        if matches.is_empty() {
            return Ok(ToolExecutionResult::success("No matches found".to_string())
                .with_data(serde_json::json!({ "count": 0, "matches": [] })));
        }

        let formatted = self.format_results(&matches, output_format);

        let summary = if count >= MAX_RESULTS {
            format!("Found {}+ matches (showing first {})", MAX_RESULTS, count)
        } else {
            format!("Found {} match(es)", count)
        };

        info!("{}", summary);

        Ok(ToolExecutionResult::success(format!("{}\n{}", summary, formatted))
            .with_data(serde_json::json!({ "count": count, "matches": matches })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grep_tool_creation() {
        let tool = GrepTool::new();
        assert_eq!(tool.name(), "grep");
    }

    #[tokio::test]
    async fn test_search_file() {
        let tool = GrepTool::new();

        // Create a temp file
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("manta_grep_test_{}.txt", uuid::Uuid::new_v4()));

        let content = r#"Line 1: Hello world
Line 2: This is a test
Line 3: Hello again
Line 4: Another line
Line 5: Hello world final"#;

        tokio::fs::write(&test_file, content).await.unwrap();

        // Search for "Hello"
        let pattern = regex::Regex::new("Hello").unwrap();
        let matches = tool.search_file(&pattern, &test_file, 1).await.unwrap();

        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].line_number, 1);
        assert_eq!(matches[1].line_number, 3);
        assert_eq!(matches[2].line_number, 5);

        // Cleanup
        let _ = tokio::fs::remove_file(&test_file).await;
    }

    #[test]
    fn test_matches_pattern() {
        let tool = GrepTool::new();
        let path = PathBuf::from("/home/user/test.rs");

        assert!(tool.matches_pattern(&path, "*.rs"));
        assert!(tool.matches_pattern(&path, ".rs"));
        assert!(tool.matches_pattern(&path, "test.rs"));
        assert!(!tool.matches_pattern(&path, "*.py"));
    }

    #[test]
    fn test_is_binary() {
        let tool = GrepTool::new();
        assert!(tool.is_binary(b"Hello\x00World"));
        assert!(!tool.is_binary(b"Hello World"));
    }
}
