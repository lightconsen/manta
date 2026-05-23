//! Browser automation tool for Manta
//!
//! Provides web browser automation capabilities using headless Chrome/Chromium.
//! Supports navigation, clicking, form input, screenshots, and content extraction.

use super::{Tool, ToolContext, ToolExecutionResult};
use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tracing::{debug, info};

#[cfg(feature = "browser")]
use chromiumoxide::page::ScreenshotParamsBuilder;

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
    /// Get all cookies for the current page
    GetCookies,
    /// Set a cookie
    SetCookie {
        name: String,
        value: String,
        domain: Option<String>,
        path: Option<String>,
    },
    /// Clear all cookies
    ClearCookies,
    /// Print page to PDF
    PrintToPdf {
        landscape: Option<bool>,
        display_header_footer: Option<bool>,
        print_background: Option<bool>,
        scale: Option<f64>,
        paper_width: Option<f64>,
        paper_height: Option<f64>,
        margin_top: Option<f64>,
        margin_bottom: Option<f64>,
        margin_left: Option<f64>,
        margin_right: Option<f64>,
        page_ranges: Option<String>,
    },
    /// Get performance metrics
    GetPerformanceMetrics,
    /// Get network request log via Performance API
    GetNetworkLog,
    /// Set mobile device emulation
    EmulateMobile { device_name: String },
    /// Set viewport size dynamically
    SetViewport {
        width: u32,
        height: u32,
        device_scale_factor: Option<f64>,
        mobile: Option<bool>,
    },
    /// Take an ARIA snapshot of the current page
    Snapshot {
        max_chars: Option<usize>,
    },
    /// Act on an element by ref_id from a previous snapshot
    #[cfg(feature = "browser")]
    Act {
        ref_id: usize,
        action: crate::browser::ActKind,
    },
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
    #[allow(dead_code)]
    default_timeout: Duration,
    /// Optional browser pool for persistent sessions
    #[cfg(feature = "browser")]
    pool: Option<std::sync::Arc<crate::browser::BrowserPool>>,
    /// Profile name for pool-based sessions
    profile: String,
}

impl Default for BrowserTool {
    fn default() -> Self {
        Self {
            chrome_path: None,
            viewport_width: 1280,
            viewport_height: 720,
            headless: true,
            default_timeout: Duration::from_secs(30),
            #[cfg(feature = "browser")]
            pool: None,
            profile: "default".to_string(),
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

    /// Set browser pool for persistent sessions (browser feature only)
    #[cfg(feature = "browser")]
    pub fn with_pool(mut self, pool: std::sync::Arc<crate::browser::BrowserPool>) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Set profile name for pool-based sessions
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = profile.into();
        self
    }

    /// Execute a single browser action against a page
    #[cfg(feature = "browser")]
    async fn execute_single_action(
        action: BrowserAction,
        page: &chromiumoxide::Page,
        screenshot_data: &mut Option<String>,
    ) -> Result<serde_json::Value, String> {
        use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;

        match action {
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
                let result = match selector {
                    Some(sel) => {
                        match page.find_element(&sel).await {
                            Ok(elem) => elem.screenshot(CaptureScreenshotFormat::Png).await,
                            Err(e) => Err(e),
                        }
                    }
                    None => {
                        let params = if full_page.unwrap_or(false) {
                            ScreenshotParamsBuilder::default()
                                .format(CaptureScreenshotFormat::Png)
                                .full_page(true)
                                .build()
                        } else {
                            ScreenshotParamsBuilder::default()
                                .format(CaptureScreenshotFormat::Png)
                                .build()
                        };
                        page.screenshot(params).await
                    }
                };

                match result {
                    Ok(data) => {
                        let base64 = base64::engine::general_purpose::STANDARD.encode(&data);
                        *screenshot_data = Some(base64.clone());
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

                match page.evaluate(script.as_str()).await {
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
                match page.evaluate(format!("() => {{ {} }}", script).as_str()).await {
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

            BrowserAction::Back => {
                let script = r#"() => { history.back(); return location.href; }"#;
                match page.evaluate(script).await {
                    Ok(result) => {
                        let url = result.into_value::<String>().unwrap_or_default();
                        Ok(json!({ "success": true, "action": "back", "url": url }))
                    }
                    Err(e) => Err(format!("Failed to go back: {}", e)),
                }
            }

            BrowserAction::Forward => {
                let script = r#"() => { history.forward(); return location.href; }"#;
                match page.evaluate(script).await {
                    Ok(result) => {
                        let url = result.into_value::<String>().unwrap_or_default();
                        Ok(json!({ "success": true, "action": "forward", "url": url }))
                    }
                    Err(e) => Err(format!("Failed to go forward: {}", e)),
                }
            }

            BrowserAction::Reload => match page.reload().await {
                Ok(_) => Ok(json!({ "success": true, "action": "reload" })),
                Err(e) => Err(format!("Failed to reload: {}", e)),
            },

            BrowserAction::GetCookies => {
                let script = r#"() => {
                    return document.cookie.split(';').map(c => {
                        const [name, ...rest] = c.trim().split('=');
                        return { name: name.trim(), value: rest.join('=') };
                    }).filter(c => c.name);
                }"#;
                match page.evaluate(script).await {
                    Ok(result) => {
                        let cookies = result.into_value::<Vec<Value>>().unwrap_or_default();
                        Ok(json!({
                            "success": true,
                            "cookies": cookies,
                            "count": cookies.len()
                        }))
                    }
                    Err(e) => Err(format!("Failed to get cookies: {}", e)),
                }
            }

            BrowserAction::SetCookie { name, value, domain, path } => {
                let domain_part = domain
                    .as_ref()
                    .map(|d| format!("domain={};", d))
                    .unwrap_or_default();
                let path_part = path
                    .as_ref()
                    .map(|p| format!("path={};", p))
                    .unwrap_or_default();
                let cookie_str = format!("{}={};{}{}", name, value, domain_part, path_part);
                let script =
                    format!(r#"() => {{ document.cookie = "{}"; return true; }}"#, cookie_str);
                match page.evaluate(script.as_str()).await {
                    Ok(_) => Ok(json!({
                        "success": true,
                        "name": name,
                        "value": value
                    })),
                    Err(e) => Err(format!("Failed to set cookie: {}", e)),
                }
            }

            BrowserAction::ClearCookies => {
                let script = r#"() => {
                    document.cookie.split(';').forEach(c => {
                        const [name] = c.split('=');
                        document.cookie = name.trim() + '=; expires=Thu, 01 Jan 1970 00:00:00 UTC; path=/';
                    });
                    return document.cookie === '';
                }"#;
                match page.evaluate(script).await {
                    Ok(result) => {
                        let cleared = result.into_value::<bool>().unwrap_or(false);
                        Ok(json!({ "success": true, "cleared": cleared }))
                    }
                    Err(e) => Err(format!("Failed to clear cookies: {}", e)),
                }
            }

            BrowserAction::PrintToPdf {
                landscape,
                display_header_footer,
                print_background,
                scale,
                paper_width,
                paper_height,
                margin_top,
                margin_bottom,
                margin_left,
                margin_right,
                page_ranges,
            } => {
                use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;

                let mut params = PrintToPdfParams::default();
                if let Some(v) = landscape {
                    params.landscape = Some(v);
                }
                if let Some(v) = display_header_footer {
                    params.display_header_footer = Some(v);
                }
                if let Some(v) = print_background {
                    params.print_background = Some(v);
                }
                if let Some(v) = scale {
                    params.scale = Some(v);
                }
                if let Some(v) = paper_width {
                    params.paper_width = Some(v);
                }
                if let Some(v) = paper_height {
                    params.paper_height = Some(v);
                }
                if let Some(v) = margin_top {
                    params.margin_top = Some(v);
                }
                if let Some(v) = margin_bottom {
                    params.margin_bottom = Some(v);
                }
                if let Some(v) = margin_left {
                    params.margin_left = Some(v);
                }
                if let Some(v) = margin_right {
                    params.margin_right = Some(v);
                }
                if let Some(ref v) = page_ranges {
                    params.page_ranges = Some(v.clone());
                }

                match page.pdf(params).await {
                    Ok(data) => {
                        let base64 = base64::engine::general_purpose::STANDARD.encode(&data);
                        Ok(json!({
                            "success": true,
                            "format": "pdf",
                            "base64_length": base64.len(),
                            "data": format!("data:application/pdf;base64,{}", base64)
                        }))
                    }
                    Err(e) => Err(format!("Failed to print PDF: {}", e)),
                }
            }

            BrowserAction::GetPerformanceMetrics => {
                let script = r#"() => {
                    const nav = performance.getEntriesByType('navigation')[0] || {};
                    return {
                        navigation: {
                            dns_lookup: nav.domainLookupEnd - nav.domainLookupStart,
                            connection_time: nav.connectEnd - nav.connectStart,
                            response_time: nav.responseEnd - nav.responseStart,
                            dom_interactive: nav.domInteractive,
                            dom_complete: nav.domComplete,
                            load_event: nav.loadEventEnd - nav.loadEventStart,
                            transfer_size: nav.transferSize,
                            decoded_body_size: nav.decodedBodySize
                        },
                        memory: performance.memory ? {
                            used_js_heap_size: performance.memory.usedJSHeapSize,
                            total_js_heap_size: performance.memory.totalJSHeapSize,
                            js_heap_size_limit: performance.memory.jsHeapSizeLimit
                        } : null
                    };
                }"#;
                match page.evaluate(script).await {
                    Ok(result) => {
                        let metrics = result.value().cloned().unwrap_or(json!(null));
                        Ok(json!({ "success": true, "metrics": metrics }))
                    }
                    Err(e) => Err(format!("Failed to get performance metrics: {}", e)),
                }
            }

            BrowserAction::GetNetworkLog => {
                let script = r#"() => {
                    return performance.getEntriesByType('resource').map(r => ({
                        name: r.name,
                        initiator_type: r.initiatorType,
                        duration: r.duration,
                        transfer_size: r.transferSize,
                        encoded_body_size: r.encodedBodySize,
                        decoded_body_size: r.decodedBodySize,
                        start_time: r.startTime
                    }));
                }"#;
                match page.evaluate(script).await {
                    Ok(result) => {
                        let entries = result.into_value::<Vec<Value>>().unwrap_or_default();
                        Ok(json!({
                            "success": true,
                            "entries": entries,
                            "count": entries.len()
                        }))
                    }
                    Err(e) => Err(format!("Failed to get network log: {}", e)),
                }
            }

            BrowserAction::EmulateMobile { device_name } => {
                let (width, height, dpr, mobile, ua) = match device_name.to_lowercase().as_str() {
                    "iphone_x" | "iphonex" => (375, 812, 3.0, true, "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1"),
                    "iphone_12" | "iphone12" => (390, 844, 3.0, true, "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1"),
                    "pixel_5" | "pixel5" => (393, 851, 2.75, true, "Mozilla/5.0 (Linux; Android 13; Pixel 5) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/112.0.0.0 Mobile Safari/537.36"),
                    "ipad" => (810, 1080, 2.0, true, "Mozilla/5.0 (iPad; CPU OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1"),
                    _ => (375, 667, 2.0, true, "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1"),
                };

                use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
                let params = SetDeviceMetricsOverrideParams::builder()
                    .width(width as i64)
                    .height(height as i64)
                    .device_scale_factor(dpr)
                    .mobile(mobile)
                    .build()
                    .map_err(|e| format!("Failed to build viewport params: {}", e))?;

                match page.execute(params).await {
                    Ok(_) => {
                        let ua_script = format!(
                            r#"() => {{ Object.defineProperty(navigator, 'userAgent', {{ value: '{}', configurable: true }}); return true; }}"#,
                            ua
                        );
                        page.evaluate(ua_script.as_str()).await.ok();
                        Ok(json!({
                            "success": true,
                            "device": device_name,
                            "viewport": { "width": width, "height": height, "device_scale_factor": dpr, "mobile": mobile }
                        }))
                    }
                    Err(e) => Err(format!("Failed to emulate mobile device: {}", e)),
                }
            }

            BrowserAction::SetViewport {
                width,
                height,
                device_scale_factor,
                mobile,
            } => {
                use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
                let mut builder = SetDeviceMetricsOverrideParams::builder()
                    .width(width as i64)
                    .height(height as i64);
                if let Some(dpr) = device_scale_factor {
                    builder = builder.device_scale_factor(dpr);
                }
                if let Some(mob) = mobile {
                    builder = builder.mobile(mob);
                }
                let params = builder.build()
                    .map_err(|e| format!("Failed to build viewport params: {}", e))?;

                match page.execute(params).await {
                    Ok(_) => Ok(json!({
                        "success": true,
                        "viewport": { "width": width, "height": height }
                    })),
                    Err(e) => Err(format!("Failed to set viewport: {}", e)),
                }
            }

            BrowserAction::Snapshot { max_chars } => {
                let max = max_chars.unwrap_or(8000);
                match crate::browser::aria_snapshot(page, max).await {
                    Ok(snapshot) => {
                        let text = snapshot.to_text();
                        Ok(json!({
                            "success": true,
                            "snapshot": text,
                            "url": snapshot.url,
                            "title": snapshot.title,
                            "interactive_count": snapshot.interactive_count(),
                            "truncated": snapshot.truncated
                        }))
                    }
                    Err(e) => Err(format!("Failed to take ARIA snapshot: {}", e)),
                }
            }

            BrowserAction::Act { ref_id, action } => {
                match crate::browser::act_by_ref(page, ref_id, action).await {
                    Ok(msg) => Ok(json!({ "success": true, "message": msg })),
                    Err(e) => Err(format!("Failed to act on ref {}: {}", ref_id, e)),
                }
            }
        }
    }

    /// Build response from action results
    fn build_result(
        results: Vec<Result<serde_json::Value, String>>,
        screenshot_data: Option<String>,
    ) -> ToolExecutionResult {
        let success = results.iter().all(|r| r.is_ok());
        let output = serde_json::to_string_pretty(&results)
            .unwrap_or_else(|_| "Failed to serialize results".to_string());

        let mut result = ToolExecutionResult::success(output);

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

        result
    }

    /// Execute browser actions via pool (persistent session)
    #[cfg(feature = "browser")]
    async fn execute_actions_pool(
        &self,
        actions: Vec<BrowserAction>,
        pool: &std::sync::Arc<crate::browser::BrowserPool>,
    ) -> crate::Result<ToolExecutionResult> {
        // Create a new page for this action sequence
        let handle = pool.new_page(&self.profile, "about:blank").await?;
        let page = &handle.page;

        let mut results = Vec::new();
        let mut screenshot_data = None;

        for action in actions {
            debug!("Executing browser action (pool): {:?}", action);
            let result = Self::execute_single_action(action, page, &mut screenshot_data).await;
            results.push(result);
        }

        Ok(Self::build_result(results, screenshot_data))
    }

    /// Execute browser actions via legacy per-call launch
    #[cfg(feature = "browser")]
    async fn execute_actions_legacy(
        &self,
        actions: Vec<BrowserAction>,
    ) -> crate::Result<ToolExecutionResult> {
        use chromiumoxide::browser::{Browser, BrowserConfig};
        use futures_util::StreamExt;
        use std::sync::Arc;

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

        if let Some(ref path) = self.chrome_path {
            builder = builder.chrome_executable(std::path::PathBuf::from(path));
        }

        let config = builder
            .build()
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: format!("Browser configuration failed: {}", e),
                cause: None,
            })?;

        let (browser, mut handler) = Browser::launch(config).await.map_err(|e| {
            crate::error::MantaError::ExternalService {
                source: "Failed to launch Chrome/Chromium. Is it installed?".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

        let browser = Arc::new(browser);
        let _browser_clone = browser.clone();
        tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });

        let page = browser.new_page("about:blank").await.map_err(|e| {
            crate::error::MantaError::ExternalService {
                source: "Failed to create browser page".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

        let mut results = Vec::new();
        let mut screenshot_data = None;

        for action in actions {
            debug!("Executing browser action (legacy): {:?}", action);
            let result = Self::execute_single_action(action, &page, &mut screenshot_data).await;
            results.push(result);
        }

        Ok(Self::build_result(results, screenshot_data))
    }

    /// Execute browser actions
    #[cfg(feature = "browser")]
    async fn execute_actions(
        &self,
        actions: Vec<BrowserAction>,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        // Use pool if available
        if let Some(ref pool) = self.pool {
            return self.execute_actions_pool(actions, pool).await;
        }

        // Fall back to legacy per-call launch
        self.execute_actions_legacy(actions).await
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
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "GetCookies": { "type": "object", "properties": {} }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "SetCookie": {
                                        "type": "object",
                                        "properties": {
                                            "name": { "type": "string", "description": "Cookie name" },
                                            "value": { "type": "string", "description": "Cookie value" },
                                            "domain": { "type": "string", "description": "Optional cookie domain" },
                                            "path": { "type": "string", "description": "Optional cookie path" }
                                        },
                                        "required": ["name", "value"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "ClearCookies": { "type": "object", "properties": {} }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "PrintToPdf": {
                                        "type": "object",
                                        "properties": {
                                            "landscape": { "type": "boolean", "description": "Landscape orientation" },
                                            "display_header_footer": { "type": "boolean", "description": "Show header/footer" },
                                            "print_background": { "type": "boolean", "description": "Print background graphics" },
                                            "scale": { "type": "number", "description": "Scale factor (0.1-2.0)" },
                                            "paper_width": { "type": "number", "description": "Paper width in inches" },
                                            "paper_height": { "type": "number", "description": "Paper height in inches" },
                                            "margin_top": { "type": "number", "description": "Top margin in inches" },
                                            "margin_bottom": { "type": "number", "description": "Bottom margin in inches" },
                                            "margin_left": { "type": "number", "description": "Left margin in inches" },
                                            "margin_right": { "type": "number", "description": "Right margin in inches" },
                                            "page_ranges": { "type": "string", "description": "Page ranges to print (e.g., '1-5, 8, 11-13')" }
                                        }
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "GetPerformanceMetrics": { "type": "object", "properties": {} }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "GetNetworkLog": { "type": "object", "properties": {} }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "EmulateMobile": {
                                        "type": "object",
                                        "properties": {
                                            "device_name": { "type": "string", "enum": ["iphone_x", "iphone_12", "pixel_5", "ipad"], "description": "Device to emulate" }
                                        },
                                        "required": ["device_name"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "SetViewport": {
                                        "type": "object",
                                        "properties": {
                                            "width": { "type": "integer", "description": "Viewport width in pixels" },
                                            "height": { "type": "integer", "description": "Viewport height in pixels" },
                                            "device_scale_factor": { "type": "number", "description": "Device scale factor (DPR)" },
                                            "mobile": { "type": "boolean", "description": "Enable mobile emulation" }
                                        },
                                        "required": ["width", "height"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Snapshot": {
                                        "type": "object",
                                        "properties": {
                                            "max_chars": { "type": "integer", "description": "Maximum characters in snapshot (default: 8000)" }
                                        }
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Act": {
                                        "type": "object",
                                        "properties": {
                                            "ref_id": { "type": "integer", "description": "Reference ID from a previous snapshot" },
                                            "action": {
                                                "type": "object",
                                                "oneOf": [
                                                    { "type": "object", "properties": { "click": { "type": "object", "properties": {} } } },
                                                    { "type": "object", "properties": { "type": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] } } },
                                                    { "type": "object", "properties": { "hover": { "type": "object", "properties": {} } } },
                                                    { "type": "object", "properties": { "fill": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] } } }
                                                ]
                                            }
                                        },
                                        "required": ["ref_id", "action"]
                                    }
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

    #[test]
    fn test_browser_tool_default() {
        let tool = BrowserTool::default();
        assert_eq!(tool.viewport_width, 1280);
        assert_eq!(tool.viewport_height, 720);
        assert!(tool.headless);
        assert_eq!(tool.default_timeout, Duration::from_secs(30));
        assert!(tool.chrome_path.is_none());
        assert_eq!(tool.profile, "default");
    }

    #[test]
    fn test_browser_tool_with_chrome_path() {
        let tool = BrowserTool::new().with_chrome_path("/usr/bin/chrome");
        assert_eq!(tool.chrome_path, Some("/usr/bin/chrome".to_string()));
    }

    #[test]
    fn test_browser_tool_with_viewport() {
        let tool = BrowserTool::new().with_viewport(1920, 1080);
        assert_eq!(tool.viewport_width, 1920);
        assert_eq!(tool.viewport_height, 1080);
    }

    #[test]
    fn test_browser_tool_with_headless() {
        let tool = BrowserTool::new().with_headless(false);
        assert!(!tool.headless);
    }

    #[test]
    fn test_browser_tool_with_profile() {
        let tool = BrowserTool::new().with_profile("headed");
        assert_eq!(tool.profile, "headed");
    }

    #[test]
    fn test_browser_tool_timeout() {
        let tool = BrowserTool::new();
        let ctx = ToolContext::default();
        assert_eq!(tool.timeout(&ctx), Duration::from_secs(60));
    }

    #[test]
    fn test_browser_action_serialization() {
        let nav = BrowserAction::Navigate {
            url: "https://example.com".to_string(),
        };
        let json = serde_json::to_string(&nav).unwrap();
        assert!(json.contains("navigate"));
        assert!(json.contains("example.com"));

        let click = BrowserAction::Click { selector: "#btn".to_string() };
        let json = serde_json::to_string(&click).unwrap();
        assert!(json.contains("click"));

        let back = BrowserAction::Back;
        let json = serde_json::to_string(&back).unwrap();
        assert!(json.contains("back"));
    }

    #[tokio::test]
    async fn test_browser_tool_execute_empty_actions() {
        let tool = BrowserTool::new();
        let ctx = ToolContext::default();
        let result = tool.execute(json!({"actions": []}), &ctx).await.unwrap();
        assert!(!result.success);
    }

    #[test]
    #[cfg(feature = "browser")]
    fn test_browser_action_new_variants_serialization() {
        let get_cookies = BrowserAction::GetCookies;
        let json = serde_json::to_string(&get_cookies).unwrap();
        assert!(json.contains("get_cookies"));

        let set_cookie = BrowserAction::SetCookie {
            name: "session".to_string(),
            value: "abc123".to_string(),
            domain: Some(".example.com".to_string()),
            path: Some("/".to_string()),
        };
        let json = serde_json::to_string(&set_cookie).unwrap();
        assert!(json.contains("set_cookie"));
        assert!(json.contains("session"));
        assert!(json.contains("abc123"));

        let clear_cookies = BrowserAction::ClearCookies;
        let json = serde_json::to_string(&clear_cookies).unwrap();
        assert!(json.contains("clear_cookies"));

        let pdf = BrowserAction::PrintToPdf {
            landscape: Some(true),
            display_header_footer: Some(false),
            print_background: Some(true),
            scale: Some(1.5),
            paper_width: Some(8.5),
            paper_height: Some(11.0),
            margin_top: Some(0.5),
            margin_bottom: Some(0.5),
            margin_left: Some(0.5),
            margin_right: Some(0.5),
            page_ranges: Some("1-3".to_string()),
        };
        let json = serde_json::to_string(&pdf).unwrap();
        assert!(json.contains("print_to_pdf"));
        assert!(json.contains("1.5"));

        let perf = BrowserAction::GetPerformanceMetrics;
        let json = serde_json::to_string(&perf).unwrap();
        assert!(json.contains("get_performance_metrics"));

        let net = BrowserAction::GetNetworkLog;
        let json = serde_json::to_string(&net).unwrap();
        assert!(json.contains("get_network_log"));

        let mobile = BrowserAction::EmulateMobile {
            device_name: "iphone_x".to_string(),
        };
        let json = serde_json::to_string(&mobile).unwrap();
        assert!(json.contains("emulate_mobile"));
        assert!(json.contains("iphone_x"));

        let viewport = BrowserAction::SetViewport {
            width: 1920,
            height: 1080,
            device_scale_factor: Some(2.0),
            mobile: Some(false),
        };
        let json = serde_json::to_string(&viewport).unwrap();
        assert!(json.contains("set_viewport"));
        assert!(json.contains("1920"));

        let snapshot = BrowserAction::Snapshot { max_chars: Some(4000) };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("snapshot"));
        assert!(json.contains("4000"));

        let act = BrowserAction::Act {
            ref_id: 3,
            action: crate::browser::ActKind::Click,
        };
        let json = serde_json::to_string(&act).unwrap();
        assert!(json.contains("act"));
        assert!(json.contains("3"));
        assert!(json.contains("click"));
    }

    #[test]
    #[cfg(feature = "browser")]
    fn test_browser_action_deserialization_new_variants() {
        // Unit variants serialize as {"variant": null}
        let get_cookies: BrowserAction =
            serde_json::from_value(json!({"get_cookies": null})).unwrap();
        assert!(matches!(get_cookies, BrowserAction::GetCookies));

        let set_cookie: BrowserAction = serde_json::from_value(json!({
            "set_cookie": { "name": "foo", "value": "bar", "domain": "example.com" }
        }))
        .unwrap();
        assert!(
            matches!(set_cookie, BrowserAction::SetCookie { ref name, ref value, .. } if name == "foo" && value == "bar")
        );

        let pdf: BrowserAction = serde_json::from_value(json!({
            "print_to_pdf": { "landscape": true, "scale": 1.2 }
        }))
        .unwrap();
        assert!(matches!(
            pdf,
            BrowserAction::PrintToPdf {
                landscape: Some(true),
                scale: Some(1.2),
                ..
            }
        ));

        let mobile: BrowserAction = serde_json::from_value(json!({
            "emulate_mobile": { "device_name": "pixel_5" }
        }))
        .unwrap();
        assert!(
            matches!(mobile, BrowserAction::EmulateMobile { ref device_name } if device_name == "pixel_5")
        );

        let viewport: BrowserAction = serde_json::from_value(json!({
            "set_viewport": { "width": 800, "height": 600, "mobile": true }
        }))
        .unwrap();
        assert!(matches!(
            viewport,
            BrowserAction::SetViewport {
                width: 800,
                height: 600,
                mobile: Some(true),
                ..
            }
        ));

        let snapshot: BrowserAction = serde_json::from_value(json!({
            "snapshot": { "max_chars": 5000 }
        }))
        .unwrap();
        assert!(
            matches!(snapshot, BrowserAction::Snapshot { max_chars: Some(5000) })
        );

        let act: BrowserAction = serde_json::from_value(json!({
            "act": { "ref_id": 7, "action": { "click": null } }
        }))
        .unwrap();
        assert!(
            matches!(act, BrowserAction::Act { ref_id: 7, action: crate::browser::ActKind::Click })
        );

        let act_type: BrowserAction = serde_json::from_value(json!({
            "act": { "ref_id": 2, "action": { "type": { "text": "hello" } } }
        }))
        .unwrap();
        assert!(
            matches!(
                act_type,
                BrowserAction::Act { ref_id: 2, action: crate::browser::ActKind::Type { ref text } } if text == "hello"
            )
        );
    }
}
