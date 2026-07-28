//! Document report tool for Syscity
//!
//! `write_report` — saves markdown or HTML content to `~/.syscity/artifacts/`
//! and returns metadata so the frontend can render a preview card.

use async_trait::async_trait;
use serde_json::Value;
use tracing::info;

use super::{create_schema, Tool, ToolContext, ToolExecutionResult};
use crate::tools::sdk::ToolCapabilities;

/// Tool that writes a user-viewable report (markdown or HTML) to the
/// artifacts directory and returns metadata for frontend preview.
#[derive(Debug, Default)]
pub struct WriteReportTool;

impl WriteReportTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WriteReportTool {
    fn name(&self) -> &str {
        "write_report"
    }

    fn description(&self) -> &str {
        "Write a markdown or HTML report that enables a rich split-panel \
         preview in the chat UI. \
         \
         PREFER THIS OVER file_write when the user asks you to create a \
         document, report, article, essay, paper, summary, analysis, or any \
         formatted content they would want to READ rather than edit. This tool \
         saves the report to a special directory and renders it with a \
         clickable preview card in the chat — the user can then open it in a \
         side-by-side viewer. \
         \
         Use cases: research reports, industry analysis, weekly summaries, \
         technical documentation, essays, HTML newsletters, formatted articles, \
         meeting notes, whitepapers, tutorials, guides, and any long-form \
         content that benefits from a dedicated reading view. \
         \
         Do NOT use this for code files, configuration files, or data files — \
         those should use file_write."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Write a report for user preview",
            serde_json::json!({
                "content": {
                    "type": "string",
                    "description": "The full content of the report (markdown or HTML)"
                },
                "filename": {
                    "type": "string",
                    "description": "Filename for the report, e.g. \"industry-report.md\" or \"report.html\""
                },
                "title": {
                    "type": "string",
                    "description": "Display title shown in the report card (defaults to filename)"
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "html"],
                    "description": "Report format (default: markdown)",
                    "default": "markdown"
                }
            }),
            vec!["content", "filename"],
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: false,
            risk_level: crate::tools::approval::RiskLevel::Low,
            categories: vec!["document".to_string(), "content".to_string()],
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let content = args["content"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("Missing 'content' argument".to_string())
        })?;

        let filename = args["filename"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("Missing 'filename' argument".to_string())
        })?;

        let title = args["title"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| filename.to_string());

        let format = args["format"]
            .as_str()
            .unwrap_or("markdown")
            .to_string();

        // Resolve artifacts directory
        let artifacts_dir = crate::dirs::syscity_dir().join("artifacts");
        tokio::fs::create_dir_all(&artifacts_dir).await.map_err(|e| {
            crate::error::SyscityError::IoContext {
                context: "Failed to create artifacts directory".to_string(),
                source: e,
            }
        })?;

        let path = artifacts_dir.join(filename);

        // Basic path traversal protection
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !canonical.starts_with(&artifacts_dir) {
            return Err(crate::error::SyscityError::Validation(format!(
                "Invalid filename '{}': path escapes artifacts directory",
                filename
            )));
        }

        tokio::fs::write(&path, content).await.map_err(|e| {
            crate::error::SyscityError::IoContext {
                context: format!("Failed to write report '{}'", filename),
                source: e,
            }
        })?;

        let file_size = content.len();
        info!(
            "Wrote report '{filename}' ({format}, {size} bytes) to {path:?}",
            filename = filename,
            format = format,
            size = file_size,
            path = &path,
        );

        let data = serde_json::json!({
            "filename": filename,
            "title": title,
            "format": format,
            "url": format!("/api/v1/artifacts/{}", filename),
            "size": file_size,
        });

        Ok(ToolExecutionResult::success(format!(
            "Report '{title}' written as {filename} ({format}, {size} bytes)",
            title = title,
            filename = filename,
            format = format,
            size = file_size,
        ))
        .with_data(data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolContext;

    #[tokio::test]
    async fn test_write_report_basic() {
        let tool = WriteReportTool::new();
        assert_eq!(tool.name(), "write_report");

        let args = serde_json::json!({
            "content": "# Test Report\n\nHello world.",
            "filename": "test-report.md",
            "title": "Test Report",
            "format": "markdown",
        });

        let ctx = ToolContext::new("test", "test-conv");
        let result = tool.execute(args, &ctx).await.unwrap();
        assert!(result.success);
        assert!(result.data.is_some());

        let data = result.data.unwrap();
        assert_eq!(data["filename"], "test-report.md");
        assert_eq!(data["title"], "Test Report");
        assert_eq!(data["format"], "markdown");
        assert!(data["url"].as_str().unwrap().contains("test-report.md"));
    }
}
