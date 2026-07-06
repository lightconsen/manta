//! Web tools for Syscity
//!
//! Tools for fetching web content and searching the web.

use async_trait::async_trait;
use serde_json::Value;
use tracing::{debug, error, info, warn};

use super::{create_schema, Tool, ToolContext, ToolExecutionResult};
use crate::tools::sdk::ToolCapabilities;

/// Maximum content size to fetch (100KB)
const MAX_CONTENT_SIZE: usize = 100 * 1024;

/// Default timeout for web requests
const WEB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Per-provider request timeout. Each provider gets a strict, shorter budget
/// so that a fallback chain has time to try more than one backend before the
/// tool-level wrapper times out.
const PROVIDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

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
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15")
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
            if let Some(end) = result[start..]
                .to_lowercase()
                .find(&pattern_end.to_lowercase())
            {
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
                        if let Some(content_end) =
                            result[content_start..].to_lowercase().find("</a>")
                        {
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
        "Fetch content from a URL. Supports HTML to markdown conversion. Maximum content size: \
         100KB."
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

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: false,
            risk_level: crate::tools::approval::RiskLevel::Medium,
            categories: vec!["network".to_string(), "web".to_string()],
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let url = args["url"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("Missing 'url' argument".to_string())
        })?;

        info!("Fetching URL: {}", url);

        // Validate URL
        let parsed_url = reqwest::Url::parse(url)
            .map_err(|e| crate::error::SyscityError::Validation(format!("Invalid URL: {}", e)))?;

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
                return Ok(ToolExecutionResult::error(format!("Failed to fetch URL: {}", e)));
            }
        };

        // Check status
        if !response.status().is_success() {
            return Ok(ToolExecutionResult::error(format!("HTTP error: {}", response.status())));
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
                return Ok(ToolExecutionResult::error(format!("Failed to read response: {}", e)));
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

        info!("Successfully fetched {} bytes from {}", truncated.len(), url);

        Ok(ToolExecutionResult::success(truncated).with_data(serde_json::json!({
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
    /// Search providers to try in order (fallback).
    /// Wrapped in Arc<RwLock<>> so hot-reload can update providers without
    /// rebuilding the entire tool registry.
    providers: std::sync::Arc<tokio::sync::RwLock<Vec<SearchProvider>>>,
}

/// Search provider configuration
#[derive(Debug, Clone, Default)]
pub enum SearchProvider {
    /// DuckDuckGo (HTML scraping)
    #[default]
    DuckDuckGo,
    /// Bing Web Search API (requires key)
    /// https://www.microsoft.com/en-us/bing/apis/bing-web-search-api
    Bing { api_key: String, endpoint: String },
    /// Google Custom Search JSON API (requires key and cx)
    /// https://developers.google.com/custom-search/v1/overview
    Google { api_key: String, cx: String },
    /// Brave Search API (requires key)
    /// https://brave.com/search/api/
    Brave { api_key: String },
    /// Custom search provider
    Custom {
        url: String,
        api_key: Option<String>,
        headers: Option<std::collections::HashMap<String, String>>,
        result_parser: Option<fn(&str, usize) -> Vec<SearchResult>>,
    },
    /// Tavily AI Search API (requires key)
    /// https://docs.tavily.com/
    Tavily { api_key: String },
    /// SerpAPI Google Search API (requires key)
    /// https://serpapi.com/
    SerpApi { api_key: String },
    /// Exa (formerly Metaphor) AI Search API (requires key)
    /// https://docs.exa.ai/
    Exa { api_key: String },
    /// Firecrawl Search API (requires key)
    /// https://docs.firecrawl.dev/
    Firecrawl { api_key: String },
}

impl Default for WebSearchTool {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .timeout(WEB_TIMEOUT)
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15")
            .build()
            .unwrap_or_default();

        Self {
            client,
            providers: std::sync::Arc::new(tokio::sync::RwLock::new(vec![
                SearchProvider::DuckDuckGo,
            ])),
        }
    }
}

impl WebSearchTool {
    /// Create a new web search tool
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a single search provider
    pub fn with_provider(mut self, provider: SearchProvider) -> Self {
        self.providers = std::sync::Arc::new(tokio::sync::RwLock::new(vec![provider]));
        self
    }

    /// Set multiple search providers to try in order
    pub fn with_providers(mut self, providers: Vec<SearchProvider>) -> Self {
        self.providers = std::sync::Arc::new(tokio::sync::RwLock::new(providers));
        self
    }

    /// Replace the provider list at runtime (used by hot-reload).
    pub async fn set_providers(&self, providers: Vec<SearchProvider>) {
        let mut guard = self.providers.write().await;
        *guard = providers;
    }

    /// Use a pre-built, shared provider list so the registry and the tool
    /// observe the same providers during hot-reload.
    pub fn with_providers_arc(
        mut self,
        providers: std::sync::Arc<tokio::sync::RwLock<Vec<SearchProvider>>>,
    ) -> Self {
        self.providers = providers;
        self
    }
}

/// Return a human-readable provider name for logging.
fn provider_name(provider: &SearchProvider) -> &'static str {
    match provider {
        SearchProvider::DuckDuckGo => "duckduckgo",
        SearchProvider::Bing { .. } => "bing",
        SearchProvider::Google { .. } => "google",
        SearchProvider::Brave { .. } => "brave",
        SearchProvider::Tavily { .. } => "tavily",
        SearchProvider::SerpApi { .. } => "serpapi",
        SearchProvider::Exa { .. } => "exa",
        SearchProvider::Firecrawl { .. } => "firecrawl",
        SearchProvider::Custom { .. } => "custom",
    }
}

impl WebSearchTool {
    /// Execute a search against a single provider.
    /// Each provider request is wrapped with its own timeout so that a slow
    /// backend does not consume the entire tool-level budget.
    async fn search_with_provider(
        &self,
        provider: &SearchProvider,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        let provider_name = provider_name(provider);
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(PROVIDER_TIMEOUT, async {
            match provider {
                SearchProvider::DuckDuckGo => self.search_duckduckgo(query, limit).await,
                SearchProvider::Bing { api_key, endpoint } => {
                    self.search_bing(api_key, endpoint, query, limit).await
                }
                SearchProvider::Google { api_key, cx } => {
                    self.search_google(api_key, cx, query, limit).await
                }
                SearchProvider::Brave { api_key } => self.search_brave(api_key, query, limit).await,
                SearchProvider::Tavily { api_key } => self.search_tavily(api_key, query, limit).await,
                SearchProvider::SerpApi { api_key } => self.search_serpapi(api_key, query, limit).await,
                SearchProvider::Exa { api_key } => self.search_exa(api_key, query, limit).await,
                SearchProvider::Firecrawl { api_key } => {
                    self.search_firecrawl(api_key, query, limit).await
                }
                SearchProvider::Custom {
                    url,
                    api_key,
                    headers,
                    result_parser,
                } => {
                    self.search_custom(url, api_key, headers, result_parser, query, limit)
                        .await
                }
            }
        })
        .await
        .map_err(|_| {
            crate::error::SyscityError::Timeout(format!(
                "Provider '{}' search exceeded {:?}",
                provider_name, PROVIDER_TIMEOUT
            ))
        })?;

        debug!(
            "Provider {} search completed in {:?}",
            provider_name,
            start.elapsed()
        );
        result
    }

    /// Search using DuckDuckGo
    async fn search_duckduckgo(
        &self,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        // Try primary endpoint first, fall back to alternative
        let encoded = urlencoding::encode(query);
        let primary_url = format!("https://html.duckduckgo.com/html/?q={}", encoded);
        let fallback_url = format!("https://lite.duckduckgo.com/lite/?q={}", encoded);

        let response = self
            .client
            .get(&primary_url)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .header("Accept-Encoding", "gzip, deflate")
            .header("DNT", "1")
            .header("Connection", "keep-alive")
            .header("Upgrade-Insecure-Requests", "1")
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Site", "none")
            .header("Sec-Fetch-User", "?1")
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await;

        let response = match response {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                debug!("DDG primary returned HTTP {}, trying fallback", r.status());
                self.client
                    .get(&fallback_url)
                    .header("Accept", "text/html")
                    .header("Accept-Language", "zh-CN,zh;q=0.9")
                    .timeout(std::time::Duration::from_secs(60))
                    .send()
                    .await
                    .map_err(|e| {
                        crate::error::SyscityError::Internal(format!(
                            "Search request failed: {}",
                            e
                        ))
                    })?
            }
            Err(_) => {
                debug!("DDG primary connection failed, trying fallback endpoint");
                self.client
                    .get(&fallback_url)
                    .header("Accept", "text/html")
                    .header("Accept-Language", "zh-CN,zh;q=0.9")
                    .timeout(std::time::Duration::from_secs(60))
                    .send()
                    .await
                    .map_err(|e| {
                        crate::error::SyscityError::Internal(format!(
                            "Search request failed: {}",
                            e
                        ))
                    })?
            }
        };

        let html = response.text().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to read response: {}", e))
        })?;

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
                let snippet =
                    if let Some(snippet_start) = chunk.find("<a class=\"result__snippet\"") {
                        let snippet_area = &chunk[snippet_start..];
                        if let Some(tag_end) = snippet_area.find(">") {
                            let content_start = tag_end + 1;
                            if let Some(content_end) = snippet_area[content_start..].find("</a>") {
                                Self::clean_html(
                                    &snippet_area[content_start..content_start + content_end],
                                )
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                results.push(SearchResult { title, url, snippet });
            }
        }

        results
    }

    /// Search using Bing Web Search API
    async fn search_bing(
        &self,
        api_key: &str,
        endpoint: &str,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        let url = format!(
            "{}/v7.0/search?q={}&count={}",
            endpoint.trim_end_matches('/'),
            urlencoding::encode(query),
            limit.min(50)
        );

        let response = self
            .client
            .get(&url)
            .header("Ocp-Apim-Subscription-Key", api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!("Bing search request failed: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(crate::error::SyscityError::Internal(format!(
                "Bing search failed: HTTP {}",
                response.status()
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to parse Bing response: {}", e))
        })?;

        let mut results = Vec::new();

        // Parse Bing API response
        if let Some(web_pages) = json.get("webPages").and_then(|wp| wp.get("value")) {
            if let Some(items) = web_pages.as_array() {
                for item in items.iter().take(limit) {
                    let title = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let url = item
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let snippet = item
                        .get("snippet")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !title.is_empty() && !url.is_empty() {
                        results.push(SearchResult { title, url, snippet });
                    }
                }
            }
        }

        Ok(results)
    }

    /// Search using Google Custom Search JSON API
    async fn search_google(
        &self,
        api_key: &str,
        cx: &str,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        let url = format!(
            "https://www.googleapis.com/customsearch/v1?key={}&cx={}&q={}&num={}",
            api_key,
            cx,
            urlencoding::encode(query),
            limit.min(10)
        );

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!("Google search request failed: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(crate::error::SyscityError::Internal(format!(
                "Google search failed: HTTP {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to parse Google response: {}", e))
        })?;

        let mut results = Vec::new();

        // Parse Google Custom Search response
        if let Some(items) = json.get("items").and_then(|v| v.as_array()) {
            for item in items.iter().take(limit) {
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let url = item
                    .get("link")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let snippet = item
                    .get("snippet")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if !title.is_empty() && !url.is_empty() {
                    results.push(SearchResult { title, url, snippet });
                }
            }
        }

        Ok(results)
    }

    /// Search using Brave Search API
    async fn search_brave(
        &self,
        api_key: &str,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}&offset=0",
            urlencoding::encode(query),
            limit.min(20)
        );

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", api_key)
            .send()
            .await
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!("Brave search request failed: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(crate::error::SyscityError::Internal(format!(
                "Brave search failed: HTTP {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let json: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to parse Brave response: {}", e))
        })?;

        let mut results = Vec::new();

        // Parse Brave Search API response
        if let Some(web) = json.get("web").and_then(|w| w.get("results")) {
            if let Some(items) = web.as_array() {
                for item in items.iter().take(limit) {
                    let title = item
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let url = item
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let snippet = item
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !title.is_empty() && !url.is_empty() {
                        results.push(SearchResult { title, url, snippet });
                    }
                }
            }
        }

        Ok(results)
    }

    /// Search using Tavily AI Search API
    async fn search_tavily(
        &self,
        api_key: &str,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        let start = std::time::Instant::now();
        debug!("Tavily search starting for query: {}", query);

        let body = serde_json::json!({
            "query": query,
            "search_depth": "basic",
            "max_results": limit.min(20),
        });

        let request_start = std::time::Instant::now();
        let response = self
            .client
            .post("https://api.tavily.com/search")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!("Tavily search failed: {}", e))
            })?;
        debug!("Tavily request sent and response received in {:?}", request_start.elapsed());

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(crate::error::SyscityError::Internal(format!(
                "Tavily search failed: HTTP {} - {}",
                status, body_text
            )));
        }

        let parse_start = std::time::Instant::now();
        let data: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to parse Tavily response: {}", e))
        })?;
        debug!("Tavily response parsed in {:?}", parse_start.elapsed());

        let mut results = Vec::new();
        if let Some(items) = data["results"].as_array() {
            for item in items.iter().take(limit) {
                results.push(SearchResult {
                    title: item["title"].as_str().unwrap_or("").to_string(),
                    url: item["url"].as_str().unwrap_or("").to_string(),
                    snippet: item["content"].as_str().unwrap_or("").to_string(),
                });
            }
        }

        debug!("Tavily search completed in {:?} with {} results", start.elapsed(), results.len());
        Ok(results)
    }

    /// Search using SerpAPI Google Search API
    async fn search_serpapi(
        &self,
        api_key: &str,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        let url = format!(
            "https://serpapi.com/search?q={}&api_key={}&engine=google",
            urlencoding::encode(query),
            urlencoding::encode(api_key),
        );

        let response = self.client.get(&url).send().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("SerpAPI search failed: {}", e))
        })?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(crate::error::SyscityError::Internal(format!(
                "SerpAPI search failed: HTTP {} - {}",
                status, body_text
            )));
        }

        let data: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to parse SerpAPI response: {}", e))
        })?;

        let mut results = Vec::new();
        if let Some(items) = data["organic_results"].as_array() {
            for item in items.iter().take(limit) {
                results.push(SearchResult {
                    title: item["title"].as_str().unwrap_or("").to_string(),
                    url: item["link"].as_str().unwrap_or("").to_string(),
                    snippet: item["snippet"].as_str().unwrap_or("").to_string(),
                });
            }
        }

        Ok(results)
    }

    /// Search using Exa (formerly Metaphor) AI Search API
    async fn search_exa(
        &self,
        api_key: &str,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        let body = serde_json::json!({
            "query": query,
            "num_results": limit.min(20),
        });

        let response = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!("Exa search failed: {}", e))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(crate::error::SyscityError::Internal(format!(
                "Exa search failed: HTTP {} - {}",
                status, body_text
            )));
        }

        let data: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to parse Exa response: {}", e))
        })?;

        let mut results = Vec::new();
        if let Some(items) = data["results"].as_array() {
            for item in items.iter().take(limit) {
                results.push(SearchResult {
                    title: item["title"].as_str().unwrap_or("").to_string(),
                    url: item["url"].as_str().unwrap_or("").to_string(),
                    snippet: item["snippet"].as_str().unwrap_or("").to_string(),
                });
            }
        }

        Ok(results)
    }

    /// Search using Firecrawl Search API
    async fn search_firecrawl(
        &self,
        api_key: &str,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        let body = serde_json::json!({
            "query": query,
            "maxResults": limit.min(20),
        });

        let response = self
            .client
            .post("https://api.firecrawl.dev/v1/search")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!("Firecrawl search failed: {}", e))
            })?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(crate::error::SyscityError::Internal(format!(
                "Firecrawl search failed: HTTP {} - {}",
                status, body_text
            )));
        }

        let data: serde_json::Value = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "Failed to parse Firecrawl response: {}",
                e
            ))
        })?;

        let mut results = Vec::new();
        if let Some(items) = data["data"].as_array() {
            for item in items.iter().take(limit) {
                results.push(SearchResult {
                    title: item["title"].as_str().unwrap_or("").to_string(),
                    url: item["url"].as_str().unwrap_or("").to_string(),
                    snippet: item["description"].as_str().unwrap_or("").to_string(),
                });
            }
        }

        Ok(results)
    }

    /// Search using custom provider
    async fn search_custom(
        &self,
        url: &str,
        api_key: &Option<String>,
        headers: &Option<std::collections::HashMap<String, String>>,
        parser: &Option<fn(&str, usize) -> Vec<SearchResult>>,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<SearchResult>> {
        // Replace placeholders in URL
        let url = url.replace("{query}", &urlencoding::encode(query));
        let url = url.replace("{limit}", &limit.to_string());

        let mut request = self.client.get(&url);

        // Add API key if provided
        if let Some(key) = api_key {
            request = request.header("Authorization", format!("Bearer {}", key));
        }

        // Add custom headers if provided
        if let Some(hdrs) = headers {
            for (key, value) in hdrs {
                request = request.header(key, value);
            }
        }

        let response = request.send().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Custom search request failed: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(crate::error::SyscityError::Internal(format!(
                "Custom search failed: HTTP {}",
                response.status()
            )));
        }

        let body = response.text().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to read response: {}", e))
        })?;

        // Use custom parser if provided, otherwise try to parse as JSON
        let results = if let Some(parser_fn) = parser {
            parser_fn(&body, limit)
        } else {
            // Default JSON parsing - assumes format similar to { "results": [{ "title":
            // "...", "url": "...", "snippet": "..." }] }
            Self::parse_generic_json_results(&body, limit)
        };

        Ok(results)
    }

    /// Parse generic JSON search results
    fn parse_generic_json_results(json: &str, limit: usize) -> Vec<SearchResult> {
        let mut results = Vec::new();

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
            // Try common result paths
            let results_array = value
                .get("results")
                .and_then(|v| v.as_array())
                .or_else(|| value.get("items").and_then(|v| v.as_array()))
                .or_else(|| value.get("data").and_then(|v| v.as_array()));

            if let Some(items) = results_array {
                for item in items.iter().take(limit) {
                    let title = item
                        .get("title")
                        .or_else(|| item.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let url = item
                        .get("url")
                        .or_else(|| item.get("link"))
                        .or_else(|| item.get("href"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let snippet = item
                        .get("snippet")
                        .or_else(|| item.get("description"))
                        .or_else(|| item.get("summary"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !title.is_empty() && !url.is_empty() {
                        results.push(SearchResult { title, url, snippet });
                    }
                }
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

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: false,
            risk_level: crate::tools::approval::RiskLevel::Low,
            categories: vec!["network".to_string(), "web".to_string()],
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let query = args["query"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("Missing 'query' argument".to_string())
        })?;

        let limit = args["limit"]
            .as_u64()
            .map(|l| l as usize)
            .unwrap_or(5)
            .clamp(1, 10);

        if query.len() > 500 {
            return Ok(ToolExecutionResult::error(
                "Query too long (max 500 characters)".to_string(),
            ));
        }

        info!("Searching for: {}", query);

        let execute_start = std::time::Instant::now();
        let providers = self.providers.read().await.clone();
        let mut last_error = None;
        for (idx, provider) in providers.iter().enumerate() {
            let provider_name = provider_name(provider);
            info!(
                "Trying provider {} ({}): {}",
                idx + 1,
                providers.len(),
                provider_name
            );
            match self.search_with_provider(provider, query, limit).await {
                Ok(results) if !results.is_empty() => {
                    if idx > 0 {
                        info!(
                            "Search fallback succeeded after {} provider(s): {}",
                            idx, provider_name
                        );
                    }
                    let result_count = results.len();
                    let format_start = std::time::Instant::now();
                    let formatted: Vec<String> = results
                        .iter()
                        .enumerate()
                        .map(|(i, r)| {
                            format!("{}. {}\n   URL: {}\n   {}", i + 1, r.title, r.url, r.snippet)
                        })
                        .collect();

                    let output = formatted.join("\n\n");
                    debug!("Formatted search results in {:?}", format_start.elapsed());
                    info!(
                        "web_search completed in {:?} with {} results from {}",
                        execute_start.elapsed(),
                        result_count,
                        provider_name
                    );
                    return Ok(ToolExecutionResult::success(output).with_data(serde_json::json!({
                        "query": query,
                        "result_count": result_count,
                        "results": results.iter().map(|r| {
                            serde_json::json!({
                                "title": r.title,
                                "url": r.url,
                                "snippet": r.snippet
                            })
                        }).collect::<Vec<_>>()
                    })));
                }
                Ok(_) => {
                    debug!("Provider {} returned no results", provider_name);
                }
                Err(e) => {
                    warn!("Provider {} search failed: {}", provider_name, e);
                    last_error = Some(e);
                }
            }
        }

        info!("web_search exhausted all providers in {:?}", execute_start.elapsed());

        if let Some(e) = last_error {
            return Err(e);
        }

        Ok(ToolExecutionResult::success("No results found for the query.".to_string()))
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
