//! Browser automation tool for Manta
//!
//! Provides web browser automation capabilities using headless Chrome/Chromium.
//! Supports navigation, clicking, form input, screenshots, and content extraction.

use super::{Tool, ToolContext, ToolExecutionResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Browser action types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAction {
    /// Navigate to a URL
    Navigate { url: String },
    /// Click on an element
    Click { selector: String },
    /// Type text into an input field
    Type {
        selector: String,
        text: String,
        clear: Option<bool>,
    },
    /// Get the current page HTML
    GetHtml,
    /// Get text content of the page or specific element
    GetText { selector: Option<String> },
    /// Take a screenshot
    Screenshot {
        full_page: Option<bool>,
        selector: Option<String>,
    },
    /// Wait for an element to appear
    WaitFor {
        selector: String,
        timeout_ms: Option<u64>,
    },
    /// Scroll the page
    Scroll { direction: String, amount: u32 },
    /// Execute JavaScript
    ExecuteScript { script: String },
    /// Go back in history
    Back,
    /// Go forward in history
    Forward,
    /// Reload the page
    Reload,
}

/// Browser tool for web automation
pub struct BrowserTool {
    /// Chrome/Chromium executable path (None = auto-detect)
    chrome_path: Option<String>,
    /// Default viewport width
    viewport_width: u32,
    /// Default viewport height
    viewport_height: u32,
    /// Whether to run headless (default: true)
    headless: bool,
    /// Default timeout for operations
    default_timeout: Duration,
}

impl Default for BrowserTool {
    fn default() -> Self {
        Self {
            chrome_path: None,
            viewport_width: 1280,
            viewport_height: 720,
            headless: true,
            default_timeout: Duration::from_secs(30),
        }
    }
}

impl BrowserTool {
    /// Create a new browser tool
    pub fn new() -> Self {
        Self::default()
    }

    /// Set Chrome/Chromium executable path
    pub fn with_chrome_path(mut self, path: impl Into<String>) -> Self {
        self.chrome_path = Some(path.into());
        self
    }

    /// Set viewport size
    pub fn with_viewport(mut self, width: u32, height: u32) -> Self {
        self.viewport_width = width;
        self.viewport_height = height;
        self
    }

    /// Set headless mode
    pub fn with_headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

    /// Execute browser actions
    #[cfg(feature = "browser")]
    async fn execute_actions(
        &self,
        actions: Vec<BrowserAction>,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        use chromiumoxide::browser::{Browser, BrowserConfig};
        use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
        use std::sync::Arc;

        // Build browser config
        let mut builder = BrowserConfig::builder()
            .viewport(chromiumoxide::handler::viewport::Viewport {
                width: self.viewport_width,
                height: self.viewport_height,
                device_scale_factor: Some(1.0),
                emulating_mobile: false,
                is_landscape: true,
                has_touch: false,
            })
            .request_timeout(self.default_timeout);

        if self.headless {
            builder = builder.arg("--headless=new");
        }

        // Add Chrome path if specified
        if let Some(ref path) = self.chrome_path {
            builder = builder.chrome_executable(std::path::PathBuf::from(path));
        }

        let config = builder
            .build()
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Browser configuration failed".to_string(),
                cause: Some(Box::new(e)),
            })?;

        // Launch browser
        let (browser, mut handler) = Browser::launch(config).await.map_err(|e| {
            crate::error::MantaError::ExternalService {
                source: "Failed to launch Chrome/Chromium. Is it installed?".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

        // Spawn handler task
        let browser = Arc::new(browser);
        let browser_clone = browser.clone();
        tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });

        // Create new page
        let page = browser.new_page("about:blank").await.map_err(|e| {
            crate::error::MantaError::ExternalService {
                source: "Failed to create browser page".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

        let mut results = Vec::new();
        let mut screenshot_data = None;

        // Execute each action
        for action in actions {
            debug!("Executing browser action: {:?}", action);

            let result = match action {
                BrowserAction::Navigate { url } => {
                    info!("Navigating to: {}", url);
                    match page.goto(&url).await {
                        Ok(_) => {
                            page.wait_for_navigation().await.ok();
                            Ok(json!({
                                "success": true,
                                "url": url,
                                "title": page.get_title().await.ok().flatten().unwrap_or_default()
                            }))
                        }
                        Err(e) => Err(format!("Failed to navigate: {}", e)),
                    }
                }

                BrowserAction::Click { selector } => match page.find_element(&selector).await {
                    Ok(elem) => match elem.click().await {
                        Ok(_) => Ok(json!({
                            "success": true,
                            "selector": selector
                        })),
                        Err(e) => Err(format!("Failed to click element: {}", e)),
                    },
                    Err(e) => Err(format!("Element not found: {}", e)),
                },

                BrowserAction::Type { selector, text, clear } => {
                    match page.find_element(&selector).await {
                        Ok(elem) => {
                            if clear.unwrap_or(true) {
                                elem.click().await.ok();
                                // Triple-click to select all
                                elem.click().await.ok();
                            }
                            match elem.type_str(&text).await {
                                Ok(_) => Ok(json!({
                                    "success": true,
                                    "selector": selector,
                                    "text_length": text.len()
                                })),
                                Err(e) => Err(format!("Failed to type: {}", e)),
                            }
                        }
                        Err(e) => Err(format!("Element not found: {}", e)),
                    }
                }

                BrowserAction::GetHtml => match page.content().await {
                    Ok(html) => Ok(json!({
                        "success": true,
                        "html": html,
                        "length": html.len()
                    })),
                    Err(e) => Err(format!("Failed to get HTML: {}", e)),
                },

                BrowserAction::GetText { selector } => {
                    match selector {
                        Some(sel) => match page.find_element(&sel).await {
                            Ok(elem) => match elem.inner_text().await {
                                Ok(Some(text)) => Ok(json!({
                                    "success": true,
                                    "text": text,
                                    "selector": sel
                                })),
                                Ok(None) => Ok(json!({
                                    "success": true,
                                    "text": "",
                                    "selector": sel
                                })),
                                Err(e) => Err(format!("Failed to get text: {}", e)),
                            },
                            Err(e) => Err(format!("Element not found: {}", e)),
                        },
                        None => {
                            // Get full page text
                            let script = r#"() => document.body.innerText"#;
                            match page.evaluate(script).await {
                                Ok(result) => {
                                    let text = result.into_value::<String>().unwrap_or_default();
                                    Ok(json!({
                                        "success": true,
                                        "text": text
                                    }))
                                }
                                Err(e) => Err(format!("Failed to get page text: {}", e)),
                            }
                        }
                    }
                }

                BrowserAction::Screenshot { full_page, selector } => {
                    let format = CaptureScreenshotFormat::Png;

                    let result = match selector {
                        Some(sel) => {
                            // Screenshot specific element
                            match page.find_element(&sel).await {
                                Ok(elem) => elem.screenshot(format).await,
                                Err(e) => Err(e),
                            }
                        }
                        None => {
                            if full_page.unwrap_or(false) {
                                page.full_screen_screenshot(format).await
                            } else {
                                page.screenshot(format).await
                            }
                        }
                    };

                    match result {
                        Ok(data) => {
                            let base64 = base64::encode(&data);
                            screenshot_data = Some(base64.clone());
                            Ok(json!({
                                "success": true,
                                "format": "png",
                                "base64_length": base64.len(),
                                "data": format!("data:image/png;base64,{}", base64)
                            }))
                        }
                        Err(e) => Err(format!("Failed to take screenshot: {}", e)),
                    }
                }

                BrowserAction::WaitFor { selector, timeout_ms } => {
                    let timeout = Duration::from_millis(timeout_ms.unwrap_or(5000));
                    let start = std::time::Instant::now();

                    loop {
                        if start.elapsed() > timeout {
                            break Err(format!("Timeout waiting for element: {}", selector));
                        }

                        match page.find_element(&selector).await {
                            Ok(_) => {
                                break Ok(json!({
                                    "success": true,
                                    "selector": selector
                                }))
                            }
                            Err(_) => {
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                        }
                    }
                }

                BrowserAction::Scroll { direction, amount } => {
                    let script = format!(
                        r#"() => {{ window.scrollBy({{ {}: {} }}); return window.scrollY; }}"#,
                        if direction == "up" { "top: -" } else { "top: " },
                        amount
                    );

                    match page.evaluate(&script).await {
                        Ok(result) => {
                            let scroll_y = result.into_value::<f64>().unwrap_or(0.0);
                            Ok(json!({
                                "success": true,
                                "direction": direction,
                                "amount": amount,
                                "scroll_y": scroll_y
                            }))
                        }
                        Err(e) => Err(format!("Failed to scroll: {}", e)),
                    }
                }

                BrowserAction::ExecuteScript { script } => {
                    match page.evaluate(&format!("() => {{ {} }}", script)).await {
                        Ok(result) => {
                            let value = result.value().cloned().unwrap_or(json!(null));
                            Ok(json!({
                                "success": true,
                                "result": value
                            }))
                        }
                        Err(e) => Err(format!("Script execution failed: {}", e)),
                    }
                }

                BrowserAction::Back => match page.go_back().await {
                    Ok(_) => Ok(json!({ "success": true, "action": "back" })),
                    Err(e) => Err(format!("Failed to go back: {}", e)),
                },

                BrowserAction::Forward => match page.go_forward().await {
                    Ok(_) => Ok(json!({ "success": true, "action": "forward" })),
                    Err(e) => Err(format!("Failed to go forward: {}", e)),
                },

                BrowserAction::Reload => match page.reload().await {
                    Ok(_) => Ok(json!({ "success": true, "action": "reload" })),
                    Err(e) => Err(format!("Failed to reload: {}", e)),
                },
            };

            results.push(result);
        }

        // Close browser
        browser_clone.close().await.ok();

        // Build response
        let success = results.iter().all(|r| r.is_ok());
        let output = serde_json::to_string_pretty(&results)
            .unwrap_or_else(|_| "Failed to serialize results".to_string());

        let mut result = ToolExecutionResult::success(output);

        // Attach screenshot data if present
        if let Some(screenshot) = screenshot_data {
            result = result.with_data(json!({
                "screenshot_base64": screenshot,
                "results": results
            }));
        } else {
            result = result.with_data(json!({ "results": results }));
        }

        if !success {
            result = ToolExecutionResult::error("One or more browser actions failed");
        }

        Ok(result)
    }

    /// Fallback implementation when browser feature is not enabled
    #[cfg(not(feature = "browser"))]
    async fn execute_actions(
        &self,
        _actions: Vec<BrowserAction>,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        Ok(ToolExecutionResult::error(
            "Browser automation not available. Build with --features browser to enable.",
        ))
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Automate web browser interactions. Navigate to URLs, click elements, fill forms, \
         take screenshots, extract content, and execute JavaScript. \
         Requires Chrome/Chromium to be installed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "actions": {
                    "type": "array",
                    "description": "List of browser actions to execute in sequence",
                    "items": {
                        "type": "object",
                        "oneOf": [
                            {
                                "type": "object",
                                "properties": {
                                    "Navigate": {
                                        "type": "object",
                                        "properties": {
                                            "url": { "type": "string", "description": "URL to navigate to" }
                                        },
                                        "required": ["url"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Click": {
                                        "type": "object",
                                        "properties": {
                                            "selector": { "type": "string", "description": "CSS selector for element to click" }
                                        },
                                        "required": ["selector"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Type": {
                                        "type": "object",
                                        "properties": {
                                            "selector": { "type": "string", "description": "CSS selector for input field" },
                                            "text": { "type": "string", "description": "Text to type" },
                                            "clear": { "type": "boolean", "description": "Clear field before typing (default: true)" }
                                        },
                                        "required": ["selector", "text"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "GetHtml": {
                                        "type": "object",
                                        "properties": {}
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "GetText": {
                                        "type": "object",
                                        "properties": {
                                            "selector": { "type": "string", "description": "Optional CSS selector (omit for full page)" }
                                        }
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Screenshot": {
                                        "type": "object",
                                        "properties": {
                                            "full_page": { "type": "boolean", "description": "Capture full page (default: false)" },
                                            "selector": { "type": "string", "description": "Optional CSS selector for specific element" }
                                        }
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "WaitFor": {
                                        "type": "object",
                                        "properties": {
                                            "selector": { "type": "string", "description": "CSS selector to wait for" },
                                            "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds (default: 5000)" }
                                        },
                                        "required": ["selector"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Scroll": {
                                        "type": "object",
                                        "properties": {
                                            "direction": { "type": "string", "enum": ["up", "down"], "description": "Scroll direction" },
                                            "amount": { "type": "integer", "description": "Pixels to scroll" }
                                        },
                                        "required": ["direction", "amount"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "ExecuteScript": {
                                        "type": "object",
                                        "properties": {
                                            "script": { "type": "string", "description": "JavaScript code to execute" }
                                        },
                                        "required": ["script"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Back": { "type": "object", "properties": {} }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Forward": { "type": "object", "properties": {} }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Reload": { "type": "object", "properties": {} }
                                }
                            }
                        ]
                    }
                }
            },
            "required": ["actions"]
        })
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let actions: Vec<BrowserAction> =
            serde_json::from_value(args.get("actions").cloned().unwrap_or(json!([]))).map_err(
                |e| crate::error::MantaError::Validation(format!("Invalid browser actions: {}", e)),
            )?;

        if actions.is_empty() {
            return Ok(ToolExecutionResult::error("No browser actions specified"));
        }

        self.execute_actions(actions, context).await
    }

    fn timeout(&self, _context: &ToolContext) -> Duration {
        Duration::from_secs(60) // Browser operations can take longer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_tool_name() {
        let tool = BrowserTool::new();
        assert_eq!(tool.name(), "browser");
    }

    #[test]
    fn test_browser_tool_schema() {
        let tool = BrowserTool::new();
        let schema = tool.parameters_schema();
        assert!(schema.get("properties").is_some());
    }
}
