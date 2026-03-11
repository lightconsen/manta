//! Web tools for Manta
//!
//! Tools for fetching web content and searching the web.

use super::{Tool, ToolContext, ToolExecutionResult, create_schema};
use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, error, info, warn};

/// Maximum content size to fetch (100KB)
const MAX_CONTENT_SIZE: usize = 100 * 1024;

/// Default timeout for web requests
const WEB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Web fetch tool for HTTP requests
#[derive(Debug, Default)]
pub struct WebFetchTool {
    /// HTTP client
    client: reqwest::Client,
}

impl WebFetchTool {
    /// Create a new web fetch tool
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(WEB_TIMEOUT)
            .user_agent("Manta/0.1.0 (Personal AI Assistant)")
            .build()
            .unwrap_or_default();

        Self { client }
    }

    /// Check if content is HTML
    fn is_html(content_type: Option<&str>) -> bool {
        content_type
            .map(|ct| ct.contains("text/html") || ct.contains("application/xhtml"))
            .unwrap_or(false)
    }

    /// Simple HTML to markdown conversion
    fn html_to_markdown(html: &str) -> String {
        // Simple regex-like replacements for common HTML tags
        let mut markdown = html.to_string();

        // Remove script and style tags with content
        markdown = Self::remove_tag(&markdown, "script");
        markdown = Self::remove_tag(&markdown, "style");

        // Convert headers
        markdown = Self::replace_tag(&markdown, "h1", "# ");
        markdown = Self::replace_tag(&markdown, "h2", "## ");
        markdown = Self::replace_tag(&markdown, "h3", "### ");
        markdown = Self::replace_tag(&markdown, "h4", "#### ");
        markdown = Self::replace_tag(&markdown, "h5", "##### ");
        markdown = Self::replace_tag(&markdown, "h6", "###### ");

        // Convert formatting
        markdown = Self::replace_tag(&markdown, "strong", "**");
        markdown = Self::replace_tag(&markdown, "b", "**");
        markdown = Self::replace_tag(&markdown, "em", "_");
        markdown = Self::replace_tag(&markdown, "i", "_");
        markdown = Self::replace_tag(&markdown, "code", "`");

        // Convert paragraphs and breaks
        markdown = markdown.replace("<p>", "\n\n");
        markdown = markdown.replace("</p>", "");
        markdown = markdown.replace("<br>", "\n");
        markdown = markdown.replace("<br/>", "\n");
        markdown = markdown.replace("<br />", "\n");

        // Convert lists
        markdown = Self::replace_list_items(&markdown, "li", "- ");
        markdown = markdown.replace("<ul>", "\n");
        markdown = markdown.replace("</ul>", "");
        markdown = markdown.replace("<ol>", "\n");
        markdown = markdown.replace("</ol>", "");

        // Convert links
        markdown = Self::convert_links(&markdown);

        // Remove remaining HTML tags
        markdown = Self::strip_remaining_tags(&markdown);

        // Clean up whitespace
        markdown = markdown
            .lines()
            .map(|line| line.trim())
            .collect::<Vec<_>>()
            .join("\n");

        // Remove multiple consecutive newlines
        while markdown.contains("\n\n\n") {
            markdown = markdown.replace("\n\n\n", "\n\n");
        }

        markdown.trim().to_string()
    }

    fn remove_tag(html: &str, tag: &str) -> String {
        let pattern_start = format!("<{}[^>]*>", tag);
        let pattern_end = format!("</{}>", tag);

        let mut result = html.to_string();
        while let Some(start) = result.to_lowercase().find(&pattern_start.to_lowercase()) {
            if let Some(end) = result[start..].to_lowercase().find(&pattern_end.to_lowercase()) {
                let end_pos = start + end + pattern_end.len();
                result.replace_range(start..end_pos, "");
            } else {
                break;
            }
        }
        result
    }

    fn replace_tag(html: &str, tag: &str, replacement: &str) -> String {
        let start_tag = format!("<{}>", tag);
        let end_tag = format!("</{}>", tag);
        let close_start_tag = format!("<{}/>", tag);

        html.replace(&start_tag, replacement)
            .replace(&end_tag, replacement)
            .replace(&close_start_tag, "")
    }

    fn replace_list_items(html: &str, tag: &str, prefix: &str) -> String {
        let start_tag = format!("<{}>", tag);
        let end_tag = format!("</{}>", tag);

        let mut result = html.to_string();
        while let Some(start) = result.to_lowercase().find(&start_tag.to_lowercase()) {
            if let Some(end) = result[start..].to_lowercase().find(&end_tag.to_lowercase()) {
                let content_start = start + start_tag.len();
                let content_end = start + end;
                let content = &result[content_start..content_end];
                let replacement = format!("\n{} {}", prefix, content.trim());
                result.replace_range(start..content_end + end_tag.len(), &replacement);
            } else {
                break;
            }
        }
        result
    }

    fn convert_links(html: &str) -> String {
        let mut result = html.to_string();
        let mut search_start = 0;

        while let Some(start) = result[search_start..].to_lowercase().find("<a ") {
            let actual_start = search_start + start;
            if let Some(href_start) = result[actual_start..].to_lowercase().find("href=\"") {
                let href_pos = actual_start + href_start + 6;
                if let Some(href_end) = result[href_pos..].find('"') {
                    let url = &result[href_pos..href_pos + href_end];
                    if let Some(tag_end) = result[actual_start..].find(">") {
                        let content_start = actual_start + tag_end + 1;
                        if let Some(content_end) = result[content_start..].to_lowercase().find("</a>") {
                            let text = &result[content_start..content_start + content_end];
                            let replacement = format!("[{}]({})", text.trim(), url);
                            let full_end = content_start + content_end + 4;
                            result.replace_range(actual_start..full_end, &replacement);
                            search_start = actual_start + replacement.len();
                            continue;
                        }
                    }
                }
            }
            search_start = actual_start + 1;
        }

        result
    }

    fn strip_remaining_tags(html: &str) -> String {
        let mut result = String::new();
        let mut in_tag = false;

        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                _ if !in_tag => result.push(ch),
                _ => {}
            }
        }

        result
    }

    /// Truncate content if it exceeds the limit
    fn truncate_content(content: String) -> String {
        if content.len() > MAX_CONTENT_SIZE {
            format!(
                "{}\n\n[Content truncated: {} bytes total]",
                &content[..MAX_CONTENT_SIZE],
                content.len()
            )
        } else {
            content
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch content from a URL. Supports HTML to markdown conversion. \
         Maximum content size: 100KB."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Fetch content from a URL",
            serde_json::json!({
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                }
            }),
            vec!["url"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'url' argument".to_string()))?;

        info!("Fetching URL: {}", url);

        // Validate URL
        let parsed_url = reqwest::Url::parse(url)
            .map_err(|e| crate::error::MantaError::Validation(format!("Invalid URL: {}", e)))?;

        // Only allow HTTP and HTTPS
        if parsed_url.scheme() != "http" && parsed_url.scheme() != "https" {
            return Ok(ToolExecutionResult::error(format!(
                "Unsupported URL scheme: {}",
                parsed_url.scheme()
            )));
        }

        // Fetch content
        let response = match self.client.get(url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                error!("Failed to fetch URL: {}", e);
                return Ok(ToolExecutionResult::error(format!(
                    "Failed to fetch URL: {}",
                    e
                )));
            }
        };

        // Check status
        if !response.status().is_success() {
            return Ok(ToolExecutionResult::error(format!(
                "HTTP error: {}",
                response.status()
            )));
        }

        // Get content type (clone to avoid borrow issues)
        let content_type: Option<String> = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        debug!("Content-Type: {:?}", content_type);

        // Get content
        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to read response body: {}", e);
                return Ok(ToolExecutionResult::error(format!(
                    "Failed to read response: {}",
                    e
                )));
            }
        };

        // Convert to string
        let content = String::from_utf8_lossy(&bytes).to_string();

        // Convert HTML to markdown if needed
        let final_content = if Self::is_html(content_type.as_deref()) {
            debug!("Converting HTML to markdown");
            Self::html_to_markdown(&content)
        } else {
            content
        };

        // Truncate if needed
        let truncated = Self::truncate_content(final_content);

        info!(
            "Successfully fetched {} bytes from {}",
            truncated.len(),
            url
        );

        Ok(ToolExecutionResult::success(truncated)
            .with_data(serde_json::json!({
                "url": url,
                "content_type": content_type,
                "size": bytes.len()
            })))
    }
}

/// Web search tool
#[derive(Debug)]
pub struct WebSearchTool {
    /// HTTP client
    client: reqwest::Client,
    /// Search provider configuration
    provider: SearchProvider,
}

/// Search provider configuration
#[derive(Debug, Clone)]
pub enum SearchProvider {
    /// DuckDuckGo (HTML scraping)
    DuckDuckGo,
    /// Bing API (requires key)
    Bing { api_key: String },
    /// Custom search provider
    Custom {
        url: String,
        api_key: Option<String>,
    },
}

impl Default for WebSearchTool {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .timeout(WEB_TIMEOUT)
            .user_agent("Manta/0.1.0 (Personal AI Assistant)")
            .build()
            .unwrap_or_default();

        Self {
            client,
            provider: SearchProvider::DuckDuckGo,
        }
    }
}

impl WebSearchTool {
    /// Create a new web search tool
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the search provider
    pub fn with_provider(mut self, provider: SearchProvider) -> Self {
        self.provider = provider;
        self
    }

    /// Search using DuckDuckGo
    async fn search_duckduckgo(&self, query: &str, limit: usize) -> crate::Result<Vec<SearchResult>> {
        // DuckDuckGo HTML interface
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );

        let response = self
            .client
            .get(&url)
            .header("Accept", "text/html")
            .send()
            .await
            .map_err(|e| crate::error::MantaError::Internal(format!("Search request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(crate::error::MantaError::Internal(format!(
                "Search failed: HTTP {}",
                response.status()
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| crate::error::MantaError::Internal(format!("Failed to read response: {}", e)))?;

        // Parse results from HTML
        let results = Self::parse_duckduckgo_results(&html, limit);

        Ok(results)
    }

    /// Parse DuckDuckGo HTML results
    fn parse_duckduckgo_results(html: &str, limit: usize) -> Vec<SearchResult> {
        let mut results = Vec::new();

        // Look for result containers
        for chunk in html.split("<div class=\"result\"") {
            if results.len() >= limit {
                break;
            }

            if let Some(title_start) = chunk.find("<a rel=\"nofollow\"") {
                let title_area = &chunk[title_start..];

                // Extract URL
                let url = if let Some(href_start) = title_area.find("href=\"") {
                    let href_pos = href_start + 6;
                    if let Some(href_end) = title_area[href_pos..].find("\"") {
                        let raw_url = &title_area[href_pos..href_pos + href_end];
                        // DuckDuckGo redirects through their domain
                        if raw_url.starts_with("//duckduckgo.com/l/?") {
                            if let Some(udm_start) = raw_url.find("uddg=") {
                                let encoded = &raw_url[udm_start + 5..];
                                urlencoding::decode(encoded)
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|_| raw_url.to_string())
                            } else {
                                raw_url.to_string()
                            }
                        } else {
                            raw_url.to_string()
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };

                // Extract title
                let title = if let Some(tag_end) = title_area.find(">") {
                    let content_start = tag_end + 1;
                    if let Some(content_end) = title_area[content_start..].find("</a>") {
                        Self::clean_html(&title_area[content_start..content_start + content_end])
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };

                // Extract snippet
                let snippet = if let Some(snippet_start) = chunk.find("<a class=\"result__snippet\"") {
                    let snippet_area = &chunk[snippet_start..];
                    if let Some(tag_end) = snippet_area.find(">") {
                        let content_start = tag_end + 1;
                        if let Some(content_end) = snippet_area[content_start..].find("</a>") {
                            Self::clean_html(&snippet_area[content_start..content_start + content_end])
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                };

                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }

        results
    }

    /// Clean HTML entities and tags from text
    fn clean_html(html: &str) -> String {
        // First, strip actual HTML tags (but not entity-encoded ones)
        let mut result = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' if in_tag => in_tag = false,
                _ if !in_tag => result.push(ch),
                _ => {}
            }
        }

        // Then decode HTML entities
        result = result.replace("&amp;", "&");
        result = result.replace("&lt;", "<");
        result = result.replace("&gt;", ">");
        result = result.replace("&quot;", "\"");
        result = result.replace("&#39;", "'");
        result = result.replace("&nbsp;", " ");

        result.trim().to_string()
    }
}

/// Search result
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Result title
    pub title: String,
    /// Result URL
    pub url: String,
    /// Result snippet
    pub snippet: String,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for information. Returns a list of search results."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Search the web",
            serde_json::json!({
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 5, max: 10)",
                    "default": 5
                }
            }),
            vec!["query"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'query' argument".to_string()))?;

        let limit = args["limit"]
            .as_u64()
            .map(|l| l as usize)
            .unwrap_or(5)
            .clamp(1, 10);

        if query.len() > 500 {
            return Ok(ToolExecutionResult::error(
                "Query too long (max 500 characters)".to_string()
            ));
        }

        info!("Searching for: {}", query);

        let results = match &self.provider {
            SearchProvider::DuckDuckGo => self.search_duckduckgo(query, limit).await,
            _ => {
                // For now, only DuckDuckGo is implemented
                warn!("Search provider not fully implemented, falling back to DuckDuckGo");
                self.search_duckduckgo(query, limit).await
            }
        }?;

        if results.is_empty() {
            return Ok(ToolExecutionResult::success(
                "No results found for the query.".to_string()
            ));
        }

        // Format results
        let formatted: Vec<String> = results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                format!(
                    "{}. {}\n   URL: {}\n   {}",
                    i + 1,
                    r.title,
                    r.url,
                    r.snippet
                )
            })
            .collect();

        let output = formatted.join("\n\n");

        info!("Found {} results for query", results.len());

        Ok(ToolExecutionResult::success(output)
            .with_data(serde_json::json!({
                "query": query,
                "result_count": results.len(),
                "results": results.iter().map(|r| {
                    serde_json::json!({
                        "title": r.title,
                        "url": r.url,
                        "snippet": r.snippet
                    })
                }).collect::<Vec<_>>()
            })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_fetch_tool_creation() {
        let tool = WebFetchTool::new();
        assert_eq!(tool.name(), "web_fetch");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_web_search_tool_creation() {
        let tool = WebSearchTool::new();
        assert_eq!(tool.name(), "web_search");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_html_to_markdown() {
        let html = r#"<h1>Title</h1><p>This is <strong>bold</strong> and <em>italic</em>.</p>"#;
        let markdown = WebFetchTool::html_to_markdown(html);
        assert!(markdown.contains("# Title"));
        assert!(markdown.contains("**bold**"));
        assert!(markdown.contains("_italic_"));
    }

    #[test]
    fn test_is_html() {
        assert!(WebFetchTool::is_html(Some("text/html")));
        assert!(WebFetchTool::is_html(Some("text/html; charset=utf-8")));
        assert!(WebFetchTool::is_html(Some("application/xhtml+xml")));
        assert!(!WebFetchTool::is_html(Some("text/plain")));
        assert!(!WebFetchTool::is_html(Some("application/json")));
        assert!(!WebFetchTool::is_html(None));
    }

    #[test]
    fn test_truncate_content() {
        let long_content = "a".repeat(MAX_CONTENT_SIZE + 100);
        let truncated = WebFetchTool::truncate_content(long_content);
        assert!(truncated.contains("truncated"));
        assert!(truncated.len() <= MAX_CONTENT_SIZE + 100);
    }

    #[test]
    fn test_parse_duckduckgo_results() {
        let html = r#"
            <div class="result">
                <a rel="nofollow" href="http://example.com">Test Title</a>
                <a class="result__snippet">Test snippet here</a>
            </div>
            <div class="result">
                <a rel="nofollow" href="http://example2.com">Second Title</a>
                <a class="result__snippet">Second snippet</a>
            </div>
        "#;

        let results = WebSearchTool::parse_duckduckgo_results(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Test Title");
        assert_eq!(results[0].url, "http://example.com");
        assert_eq!(results[0].snippet, "Test snippet here");
    }

    #[test]
    fn test_clean_html() {
        assert_eq!(WebSearchTool::clean_html("Hello &amp; World"), "Hello & World");
        assert_eq!(WebSearchTool::clean_html("&lt;tag&gt;"), "<tag>");
        assert_eq!(WebSearchTool::clean_html("<b>Bold</b>"), "Bold");
    }
}
