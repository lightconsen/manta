//! PDF Tool — PDF Generation from Text/Markdown
//!
//! tool for generating PDF documents from
//! text, markdown, or HTML content. Uses headless Chrome via
//! `chromiumoxide` if available, or falls back to generating
//! an HTML file that the user can print to PDF.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tracing::{info, warn};

use super::{Tool, ToolContext, ToolExecutionResult};

/// PDF generation tool
pub struct PdfTool;

impl PdfTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PdfTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct PdfArgs {
    /// Content to convert (markdown or plain text)
    content: String,
    /// Output file path (default: current dir)
    #[serde(default)]
    output: Option<String>,
    /// Document title
    #[serde(default)]
    title: Option<String>,
    /// Page orientation: portrait or landscape
    #[serde(default = "default_orientation")]
    #[allow(dead_code)]
    orientation: String,
    /// Paper size: a4, letter, legal
    #[serde(default = "default_paper")]
    #[allow(dead_code)]
    paper: String,
}

fn default_orientation() -> String {
    "portrait".to_string()
}

fn default_paper() -> String {
    "a4".to_string()
}

/// Convert markdown-like content to a simple HTML document
fn markdown_to_html(content: &str, title: &str) -> String {
    let mut html = content.to_string();

    // Escape HTML entities
    html = html.replace('&', "&amp;");
    html = html.replace('<', "&lt;");
    html = html.replace('>', "&gt;");

    // Headers
    for level in (1..=6).rev() {
        let prefix = "#".repeat(level);
        let tag = format!("h{}", level);
        let re = regex::Regex::new(&format!(r"(?m)^{prefix} (.+)$")).unwrap();
        html = re
            .replace_all(&html, |caps: &regex::Captures<'_>| {
                format!("<{}>{}</{}>", tag, caps.get(1).unwrap().as_str(), tag)
            })
            .to_string();
    }

    // Bold: handle ** pairs correctly
    let mut result = String::new();
    let mut chars = html.chars().peekable();
    let mut in_bold = false;
    while let Some(ch) = chars.next() {
        if ch == '*' && chars.peek() == Some(&'*') {
            chars.next(); // consume second *
            if in_bold {
                result.push_str("</strong>");
                in_bold = false;
            } else {
                result.push_str("<strong>");
                in_bold = true;
            }
        } else {
            result.push(ch);
        }
    }
    html = result;

    // Code blocks
    html = html.replace("```", "<pre><code>");
    // naive: pairs
    let mut result = String::new();
    let mut in_code = false;
    for part in html.split("<pre><code>") {
        if in_code {
            if let Some(idx) = part.find("<pre><code>") {
                let (code, rest) = part.split_at(idx);
                result.push_str("<pre><code>");
                result.push_str(code);
                result.push_str("</code></pre>");
                result.push_str(rest);
                in_code = false;
            } else {
                result.push_str("<pre><code>");
                result.push_str(part);
                in_code = true;
            }
        } else {
            result.push_str(part);
            in_code = true;
        }
    }
    // Handle unclosed code blocks
    if in_code {
        result.push_str("</code></pre>");
    }
    html = result;

    // Inline code
    let re = regex::Regex::new(r"`([^`]+)`").unwrap();
    html = re
        .replace_all(&html, |caps: &regex::Captures<'_>| {
            format!("<code>{}</code>", caps.get(1).unwrap().as_str())
        })
        .to_string();

    // Line breaks
    html = html.replace("\n\n", "</p><p>");
    html = html.replace('\n', "<br>");

    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>{}</title>
<style>
body {{ font-family: system-ui, -apple-system, sans-serif; margin: 40px; line-height: 1.6; color: #333; }}
h1, h2, h3 {{ color: #1a1a1a; }}
code {{ background: #f4f4f4; padding: 2px 6px; border-radius: 3px; font-size: 0.9em; }}
pre {{ background: #f4f4f4; padding: 16px; border-radius: 6px; overflow-x: auto; }}
pre code {{ background: none; padding: 0; }}
table {{ border-collapse: collapse; width: 100%; }}
th, td {{ border: 1px solid #ddd; padding: 8px; text-align: left; }}
th {{ background: #f8f8f8; }}
</style>
</head>
<body>
<h1>{}</h1>
<p>{}</p>
</body>
</html>"#,
        title, title, html
    )
}

#[async_trait]
impl Tool for PdfTool {
    fn name(&self) -> &str {
        "pdf"
    }

    fn description(&self) -> &str {
        "Generate a PDF document from text or markdown content. Produces an HTML file that can be \
         printed to PDF, or uses a PDF engine if available."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Content to convert (markdown or plain text)"
                },
                "output": {
                    "type": "string",
                    "description": "Output file path (default: current dir / output.pdf)"
                },
                "title": {
                    "type": "string",
                    "description": "Document title"
                },
                "orientation": {
                    "type": "string",
                    "enum": ["portrait", "landscape"],
                    "default": "portrait"
                },
                "paper": {
                    "type": "string",
                    "enum": ["a4", "letter", "legal"],
                    "default": "a4"
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let start = std::time::Instant::now();
        let args: PdfArgs = match serde_json::from_value(args) {
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

        let title = args.title.as_deref().unwrap_or("Document");
        let output_path = if let Some(out) = args.output {
            std::path::PathBuf::from(out)
        } else {
            context.working_directory().join("output.pdf")
        };

        // Generate HTML as intermediate format
        let html = markdown_to_html(&args.content, title);
        let html_path = output_path.with_extension("html");

        if let Err(e) = tokio::fs::write(&html_path, &html).await {
            return Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!("Failed to write HTML: {}", e)),
                data: None,
                execution_time: start.elapsed(),
            });
        }

        // Try to convert HTML to PDF using headless Chrome/Chromium
        let pdf_result = tokio::process::Command::new("google-chrome")
            .args([
                "--headless",
                "--disable-gpu",
                "--no-sandbox",
                "--print-to-pdf",
                output_path.to_str().unwrap_or("output.pdf"),
                html_path.to_str().unwrap_or("output.html"),
            ])
            .output()
            .await;

        let (success, method) = match pdf_result {
            Ok(output) if output.status.success() => {
                info!("PDF generated via Chrome: {:?}", output_path);
                (true, "chrome")
            }
            _ => {
                // Try chromium
                let result = tokio::process::Command::new("chromium")
                    .args([
                        "--headless",
                        "--disable-gpu",
                        "--no-sandbox",
                        "--print-to-pdf",
                        output_path.to_str().unwrap_or("output.pdf"),
                        html_path.to_str().unwrap_or("output.html"),
                    ])
                    .output()
                    .await;

                match result {
                    Ok(output) if output.status.success() => {
                        info!("PDF generated via Chromium: {:?}", output_path);
                        (true, "chromium")
                    }
                    _ => {
                        // Fallback: keep HTML, user can print to PDF
                        warn!("No headless browser found; keeping HTML output");
                        (false, "html_fallback")
                    }
                }
            }
        };

        let output_file = if success {
            output_path.clone()
        } else {
            html_path.clone()
        };

        Ok(ToolExecutionResult {
            success: true, // Even fallback is "success" — we produced output
            output: if success {
                format!("PDF generated: {}", output_path.display())
            } else {
                format!(
                    "HTML generated (no headless browser found for PDF conversion): {}",
                    html_path.display()
                )
            },
            error: None,
            data: Some(serde_json::json!({
                "output_file": output_file.to_string_lossy(),
                "method": method,
                "format": if success { "pdf" } else { "html" },
                "title": title,
            })),
            execution_time: start.elapsed(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_to_html_bold() {
        let html = markdown_to_html("**bold text**", "Test");
        assert!(
            html.contains("<strong>bold text</strong>"),
            "bold tags should wrap text: {}",
            html
        );
    }

    #[test]
    fn test_markdown_to_html_headers() {
        let html = markdown_to_html("# Title\n## Subtitle", "Doc");
        assert!(html.contains("<h1>Title</h1>"), "h1 should be generated: {}", html);
        assert!(html.contains("<h2>Subtitle</h2>"), "h2 should be generated: {}", html);
    }

    #[test]
    fn test_markdown_to_html_inline_code() {
        let html = markdown_to_html("use `cargo test` to run", "Doc");
        assert!(
            html.contains("<code>cargo test</code>"),
            "inline code should be wrapped: {}",
            html
        );
    }

    #[test]
    fn test_markdown_to_html_escapes_html_entities() {
        let html = markdown_to_html("5 < 10 && 10 > 5", "Doc");
        assert!(!html.contains("5 < 10"), "raw < should be escaped");
        assert!(html.contains("&lt;"), "< should become &lt;: {}", html);
        assert!(html.contains("&gt;"), "> should become &gt;: {}", html);
    }

    #[test]
    fn test_markdown_to_html_contains_doctype_and_title() {
        let html = markdown_to_html("Hello", "My Title");
        assert!(html.contains("<!DOCTYPE html>"), "should have doctype");
        assert!(html.contains("<title>My Title</title>"), "should have title");
        assert!(html.contains("<h1>My Title</h1>"), "should have h1 title");
    }

    #[test]
    fn test_pdf_args_defaults() {
        let args: PdfArgs = serde_json::from_value(serde_json::json!({
            "content": "Hello"
        }))
        .unwrap();
        assert_eq!(args.content, "Hello");
        assert_eq!(args.orientation, "portrait");
        assert_eq!(args.paper, "a4");
        assert!(args.output.is_none());
        assert!(args.title.is_none());
    }

    #[test]
    fn test_pdf_args_custom() {
        let args: PdfArgs = serde_json::from_value(serde_json::json!({
            "content": "Hello",
            "orientation": "landscape",
            "paper": "letter",
            "title": "My Doc",
            "output": "/tmp/out.pdf"
        }))
        .unwrap();
        assert_eq!(args.orientation, "landscape");
        assert_eq!(args.paper, "letter");
        assert_eq!(args.title, Some("My Doc".to_string()));
        assert_eq!(args.output, Some("/tmp/out.pdf".to_string()));
    }
}
