//! Browser automation tool for Syscity
//!
//! Provides web browser automation capabilities using headless Chrome/Chromium.
//! Supports navigation, clicking, form input, screenshots, and content
//! extraction.

use std::time::Duration;

use async_trait::async_trait;
#[cfg(feature = "browser")]
use base64::Engine;
#[cfg(feature = "browser")]
use chromiumoxide::page::ScreenshotParamsBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(feature = "browser")]
use tracing::{debug, info, warn};

use super::{Tool, ToolContext, ToolExecutionResult};
use crate::tools::sdk::ToolCapabilities;

/// A single form field for `FillForm`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormField {
    /// CSS selector of the input
    pub selector: String,
    /// Value to type into it
    pub value: String,
    /// Clear existing content first (default: true)
    pub clear: Option<bool>,
}

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
    /// Fill multiple form fields in one action
    FillForm {
        /// Fields to fill: each has a CSS selector and a value
        fields: Vec<FormField>,
    },
    /// Hover over an element
    Hover { selector: String },
    /// Click at viewport coordinates
    ClickAt { x: f64, y: f64 },
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
    /// Get network log (fetch/XHR) with optional filtering and bodies
    GetNetworkLog {
        /// Substring filter on the request URL
        url: Option<String>,
        /// HTTP method filter (GET, POST, ...)
        method: Option<String>,
        /// Resource type filter (document, xhr, fetch, script, img, stylesheet, ...)
        resource_type: Option<String>,
        /// Minimum HTTP status code (inclusive)
        min_status: Option<u16>,
        /// Maximum HTTP status code (inclusive)
        max_status: Option<u16>,
        /// Include (truncated) response bodies, default true
        include_body: Option<bool>,
        /// Max entries to return (default 50)
        limit: Option<usize>,
        /// Entries to skip (pagination)
        offset: Option<usize>,
    },
    /// Get captured console messages and uncaught exceptions
    GetConsoleMessages {
        /// Level filter: log, info, warn, error, debug, exception
        level: Option<String>,
        /// Max entries to return (default 100)
        limit: Option<usize>,
    },
    /// Clear captured network and console buffers
    ClearCaptures,
    /// Start a screencast, saving JPEG frames to an artifacts directory
    ScreencastStart {
        /// JPEG quality 0-100 (default 80)
        quality: Option<u32>,
        /// Save every Nth frame (default 1)
        every_nth_frame: Option<u32>,
    },
    /// Stop the active screencast and return the saved frames location
    ScreencastStop,
    /// Set mobile device emulation
    EmulateMobile { device_name: String },
    /// Emulate network conditions (throttling)
    EmulateNetwork {
        /// Extra latency in ms
        latency_ms: Option<f64>,
        /// Download throughput in bytes/sec (-1 or None = no limit)
        download_bps: Option<f64>,
        /// Upload throughput in bytes/sec
        upload_bps: Option<f64>,
        /// Simulate offline mode
        offline: Option<bool>,
    },
    /// Throttle CPU by a slowdown factor (1 = no throttle, 4 = 4x slower)
    EmulateCpu { rate: f64 },
    /// Set viewport size dynamically
    SetViewport {
        width: u32,
        height: u32,
        device_scale_factor: Option<f64>,
        mobile: Option<bool>,
    },
    /// Take an ARIA snapshot of the current page
    Snapshot { max_chars: Option<usize> },
    /// Act on an element by ref_id from a previous snapshot
    #[cfg(feature = "browser")]
    Act {
        ref_id: usize,
        action: crate::browser::ActKind,
    },
    /// Press a key on the page
    Press { key: String },
    /// Drag an element from one point to another
    Drag {
        selector: String,
        target_selector: Option<String>,
        delta_x: Option<i32>,
        delta_y: Option<i32>,
    },
    /// Select text in an input or textarea
    Select {
        selector: String,
        text: Option<String>,
        start: Option<usize>,
        end: Option<usize>,
    },
    /// Upload files to a file input element
    UploadFiles {
        selector: String,
        files: Vec<String>,
    },
    /// Handle a JavaScript dialog (alert/confirm/prompt)
    HandleDialog {
        action: String,
        text: Option<String>,
    },
    /// Set download behavior
    SetDownloadBehavior {
        behavior: String,
        download_path: Option<String>,
    },
    /// List browser tabs/pages
    ListTabs,
    /// Switch to a specific tab by index or title
    SwitchTab {
        index: Option<usize>,
        title: Option<String>,
    },
    /// Close a specific tab by index or title
    CloseTab {
        index: Option<usize>,
        title: Option<String>,
    },
}

/// Normalize action names in browser action JSON values.
/// Converts PascalCase action names to snake_case for serde compatibility.
/// This handles cases where the LLM sends the Rust enum variant name
/// (e.g. "Navigate" or "GetHtml") instead of the serde-renamed snake_case
/// form (e.g. "navigate" or "get_html").
fn normalize_browser_actions(value: &mut Value) {
    let normalize_action = |name: &str| -> String {
        let mut result = String::with_capacity(name.len() + 4);
        for (i, c) in name.chars().enumerate() {
            if c.is_uppercase() {
                if i > 0 {
                    result.push('_');
                }
                for lower in c.to_lowercase() {
                    result.push(lower);
                }
            } else {
                result.push(c);
            }
        }
        result
    };

    match value {
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                if let Value::Object(obj) = item {
                    let orig = obj.get("action").and_then(|v| v.as_str()).map(String::from);
                    if let Some(name) = orig {
                        let normalized = normalize_action(&name);
                        if normalized != name {
                            obj.insert("action".to_string(), Value::String(normalized));
                        }
                    }
                }
            }
        }
        Value::Object(obj) => {
            let orig = obj.get("action").and_then(|v| v.as_str()).map(String::from);
            if let Some(name) = orig {
                let normalized = normalize_action(&name);
                if normalized != name {
                    obj.insert("action".to_string(), Value::String(normalized));
                }
            }
        }
        _ => {}
    }
}
/// State for an active screencast recording session.
#[cfg(feature = "browser")]
struct ScreencastSession {
    /// Directory receiving frame-*.jpg files
    dir: std::path::PathBuf,
    /// Number of frames written so far
    frames: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Collector task reading screencastFrame events
    task: tokio::task::JoinHandle<()>,
}

pub struct BrowserTool {
    /// Chrome/Chromium executable path (None = auto-detect)
    chrome_path: Option<String>,
    /// Default viewport width
    viewport_width: u32,
    /// Default viewport height
    viewport_height: u32,
    /// Whether to run headless (default: true)
    headless: bool,
    /// Default timeout for browser operations (feature-gated: only used when
    /// `browser` feature is enabled, hence `#[allow(dead_code)]` to suppress
    /// warnings when feature is off).
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
        browser: Option<&chromiumoxide::Browser>,
        screenshot_data: &mut Option<String>,
    ) -> Result<serde_json::Value, String> {
        use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;

        match action {
            BrowserAction::Navigate { url } => {
                info!("Navigating to: {}", url);
                match page.goto(&url).await {
                    Ok(_) => {
                        if let Err(e) = page.wait_for_navigation().await {
                            warn!("Navigation wait failed after page load: {}", e);
                        }
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
                    Ok(_) => {
                        crate::browser::instrument::auto_wait(page).await;
                        Ok(json!({
                            "success": true,
                            "selector": selector
                        }))
                    }
                    Err(e) => Err(format!("Failed to click element: {}", e)),
                },
                Err(e) => Err(format!("Element not found: {}", e)),
            },

            BrowserAction::Type { selector, text, clear } => {
                match page.find_element(&selector).await {
                    Ok(elem) => {
                        if clear.unwrap_or(true) {
                            if elem.click().await.is_err() {
                                warn!("Failed to click browser element before typing");
                            }
                            if elem.click().await.is_err() {
                                warn!("Failed to click browser element before typing");
                            }
                        }
                        match elem.type_str(&text).await {
                            Ok(_) => {
                                crate::browser::instrument::auto_wait(page).await;
                                Ok(json!({
                                    "success": true,
                                    "selector": selector,
                                    "text_length": text.len()
                                }))
                            }
                            Err(e) => Err(format!("Failed to type: {}", e)),
                        }
                    }
                    Err(e) => Err(format!("Element not found: {}", e)),
                }
            }

            BrowserAction::FillForm { fields } => {
                let mut filled = 0usize;
                let mut errors = Vec::new();
                for field in &fields {
                    match page.find_element(&field.selector).await {
                        Ok(elem) => {
                            if field.clear.unwrap_or(true) {
                                let _ = elem.click().await;
                            }
                            match elem.type_str(&field.value).await {
                                Ok(_) => filled += 1,
                                Err(e) => errors.push(format!("{}: {}", field.selector, e)),
                            }
                        }
                        Err(e) => errors.push(format!("{}: {}", field.selector, e)),
                    }
                }
                crate::browser::instrument::auto_wait(page).await;
                if errors.is_empty() {
                    Ok(json!({ "success": true, "filled": filled }))
                } else {
                    Err(format!(
                        "Filled {}/{} fields; errors: {}",
                        filled,
                        fields.len(),
                        errors.join("; ")
                    ))
                }
            }

            BrowserAction::Hover { selector } => match page.find_element(&selector).await {
                Ok(elem) => match elem.hover().await {
                    Ok(_) => Ok(json!({ "success": true, "selector": selector })),
                    Err(e) => Err(format!("Failed to hover element: {}", e)),
                },
                Err(e) => Err(format!("Element not found: {}", e)),
            },

            BrowserAction::ClickAt { x, y } => {
                use chromiumoxide::cdp::browser_protocol::input::{
                    DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
                };
                let mut press = DispatchMouseEventParams::new(
                    DispatchMouseEventType::MousePressed,
                    x,
                    y,
                );
                press.button = Some(MouseButton::Left);
                press.click_count = Some(1);
                let mut release = DispatchMouseEventParams::new(
                    DispatchMouseEventType::MouseReleased,
                    x,
                    y,
                );
                release.button = Some(MouseButton::Left);
                release.click_count = Some(1);
                if let Err(e) = page.execute(press).await {
                    return Err(format!("Failed to click at ({}, {}): {}", x, y, e));
                }
                if let Err(e) = page.execute(release).await {
                    return Err(format!("Failed to click at ({}, {}): {}", x, y, e));
                }
                crate::browser::instrument::auto_wait(page).await;
                Ok(json!({ "success": true, "x": x, "y": y }))
            }

            BrowserAction::GetHtml => match page.content().await {
                Ok(html) => Ok(json!({
                    "success": true,
                    "html": html,
                    "length": html.len()
                })),
                Err(e) => Err(format!("Failed to get HTML: {}", e)),
            },

            BrowserAction::GetText { selector } => match selector {
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
            },

            BrowserAction::Screenshot { full_page, selector } => {
                let result = match selector {
                    Some(sel) => match page.find_element(&sel).await {
                        Ok(elem) => elem.screenshot(CaptureScreenshotFormat::Png).await,
                        Err(e) => Err(e),
                    },
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
                let (dx, dy) = if direction == "up" {
                    (0, -(amount as i32))
                } else if direction == "down" {
                    (0, amount as i32)
                } else if direction == "left" {
                    (-(amount as i32), 0)
                } else {
                    (amount as i32, 0)
                };
                let script = format!(
                    r#"() => {{ window.scrollBy({}, {}); return window.scrollY; }}"#,
                    dx, dy
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
                match page
                    .evaluate(format!("() => {{ {} }}", script).as_str())
                    .await
                {
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

            BrowserAction::GetNetworkLog {
                url,
                method,
                resource_type,
                min_status,
                max_status,
                include_body,
                limit,
                offset,
            } => {
                // Prefer the CDP capture (all resource types); fall back to
                // the injected fetch/XHR shim when CDP capture is inactive.
                if crate::browser::network_log::start_capture(page).await.is_ok() {
                    return crate::browser::network_log::query(
                        page,
                        url.as_deref(),
                        method.as_deref(),
                        min_status,
                        max_status,
                        resource_type.as_deref(),
                        include_body.unwrap_or(true),
                        limit.unwrap_or(50),
                        offset.unwrap_or(0),
                    )
                    .await;
                }

                crate::browser::instrument::ensure_instrumented(page).await?;
                let script = r#"() => (window.__syscity_net || [])"#;
                match page.evaluate(script).await {
                    Ok(result) => {
                        let entries = result.into_value::<Vec<Value>>().unwrap_or_default();
                        let include_body = include_body.unwrap_or(true);
                        let filtered: Vec<Value> = entries
                            .into_iter()
                            .filter(|e| {
                                url.as_ref().is_none_or(|f| {
                                    e.get("url")
                                        .and_then(|u| u.as_str())
                                        .is_some_and(|u| u.contains(f.as_str()))
                                })
                            })
                            .filter(|e| {
                                method.as_ref().is_none_or(|m| {
                                    e.get("method")
                                        .and_then(|v| v.as_str())
                                        .is_some_and(|v| v.eq_ignore_ascii_case(m))
                                })
                            })
                            .filter(|e| {
                                let status = e.get("status").and_then(|s| s.as_u64());
                                min_status.is_none_or(|min| {
                                    status.is_some_and(|s| s >= min as u64)
                                }) && max_status.is_none_or(|max| {
                                    status.is_some_and(|s| s <= max as u64)
                                })
                            })
                            .map(|mut e| {
                                if !include_body {
                                    if let Value::Object(ref mut obj) = e {
                                        obj.remove("body");
                                        obj.remove("body_truncated");
                                    }
                                }
                                e
                            })
                            .collect();
                        let total = filtered.len();
                        let offset = offset.unwrap_or(0);
                        let page_entries: Vec<Value> = filtered
                            .into_iter()
                            .skip(offset)
                            .take(limit.unwrap_or(50))
                            .collect();
                        Ok(json!({
                            "success": true,
                            "entries": page_entries,
                            "count": page_entries.len(),
                            "total": total,
                            "offset": offset,
                            "source": "shim",
                            "note": "Captures fetch/XHR only (not document/image/css loads). Bodies truncated to 8KB."
                        }))
                    }
                    Err(e) => Err(format!("Failed to get network log: {}", e)),
                }
            }

            BrowserAction::GetConsoleMessages { level, limit } => {
                crate::browser::instrument::ensure_instrumented(page).await?;
                let script = r#"() => (window.__syscity_console || [])"#;
                match page.evaluate(script).await {
                    Ok(result) => {
                        let entries = result.into_value::<Vec<Value>>().unwrap_or_default();
                        let filtered: Vec<Value> = entries
                            .into_iter()
                            .filter(|e| {
                                level.as_ref().is_none_or(|l| {
                                    e.get("level")
                                        .and_then(|v| v.as_str())
                                        .is_some_and(|v| v.eq_ignore_ascii_case(l))
                                })
                            })
                            .collect();
                        let total = filtered.len();
                        let mut messages: Vec<Value> =
                            filtered.into_iter().take(limit.unwrap_or(100)).collect();
                        // Best-effort: resolve error/warn stacks via source maps.
                        crate::browser::sourcemap::sourcemap_messages(&mut messages).await;
                        Ok(json!({
                            "success": true,
                            "messages": messages,
                            "count": messages.len(),
                            "total": total
                        }))
                    }
                    Err(e) => Err(format!("Failed to get console messages: {}", e)),
                }
            }

            BrowserAction::ClearCaptures => {
                crate::browser::network_log::clear(page).await;
                let script = r#"() => {
                    if (window.__syscity_net) window.__syscity_net.length = 0;
                    if (window.__syscity_console) window.__syscity_console.length = 0;
                }"#;
                match page.evaluate(script).await {
                    Ok(_) => Ok(json!({ "success": true })),
                    Err(e) => Err(format!("Failed to clear captures: {}", e)),
                }
            }

            BrowserAction::EmulateNetwork {
                latency_ms,
                download_bps,
                upload_bps,
                offline,
            } => {
                // Deprecated in favor of the Emulation-domain variant in newer
                // CDP revisions, but still fully supported by Chrome.
                #[allow(deprecated)]
                use chromiumoxide::cdp::browser_protocol::network::EmulateNetworkConditionsParams;
                #[allow(deprecated)]
                let params = EmulateNetworkConditionsParams::new(
                    offline.unwrap_or(false),
                    latency_ms.unwrap_or(0.0),
                    download_bps.unwrap_or(-1.0),
                    upload_bps.unwrap_or(-1.0),
                );
                match page.execute(params).await {
                    Ok(_) => Ok(json!({
                        "success": true,
                        "latency_ms": latency_ms.unwrap_or(0.0),
                        "offline": offline.unwrap_or(false)
                    })),
                    Err(e) => Err(format!("Failed to emulate network conditions: {}", e)),
                }
            }

            BrowserAction::EmulateCpu { rate } => {
                use chromiumoxide::cdp::browser_protocol::emulation::SetCpuThrottlingRateParams;
                match page.execute(SetCpuThrottlingRateParams::new(rate.max(1.0))).await {
                    Ok(_) => Ok(json!({ "success": true, "rate": rate })),
                    Err(e) => Err(format!("Failed to set CPU throttling: {}", e)),
                }
            }

            BrowserAction::ScreencastStart {
                quality,
                every_nth_frame,
            } => Self::screencast_start(page, quality, every_nth_frame).await,

            BrowserAction::ScreencastStop => Self::screencast_stop(page).await,

            BrowserAction::EmulateMobile { device_name } => {
                let (width, height, dpr, mobile, ua) = match device_name.to_lowercase().as_str() {
                    "iphone_x" | "iphonex" => (
                        375,
                        812,
                        3.0,
                        true,
                        "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) \
                         AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 \
                         Safari/604.1",
                    ),
                    "iphone_12" | "iphone12" => (
                        390,
                        844,
                        3.0,
                        true,
                        "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) \
                         AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 \
                         Safari/604.1",
                    ),
                    "pixel_5" | "pixel5" => (
                        393,
                        851,
                        2.75,
                        true,
                        "Mozilla/5.0 (Linux; Android 13; Pixel 5) AppleWebKit/537.36 (KHTML, like \
                         Gecko) Chrome/112.0.0.0 Mobile Safari/537.36",
                    ),
                    "ipad" => (
                        810,
                        1080,
                        2.0,
                        true,
                        "Mozilla/5.0 (iPad; CPU OS 16_0 like Mac OS X) AppleWebKit/605.1.15 \
                         (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1",
                    ),
                    _ => (
                        375,
                        667,
                        2.0,
                        true,
                        "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) \
                         AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 \
                         Safari/604.1",
                    ),
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
                        if let Err(e) = page.evaluate(ua_script.as_str()).await {
                            warn!("Failed to set custom user agent: {}", e);
                        }
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
                let params = builder
                    .build()
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
                    Ok(msg) => {
                        crate::browser::instrument::auto_wait(page).await;
                        Ok(json!({ "success": true, "message": msg }))
                    }
                    Err(e) => Err(format!("Failed to act on ref {}: {}", ref_id, e)),
                }
            }

            BrowserAction::Press { key } => {
                let script = format!(
                    r#"() => {{
                        const el = document.activeElement || document.body;
                        const evt = new KeyboardEvent('keydown', {{ key: '{}', bubbles: true }});
                        el.dispatchEvent(evt);
                        const evtUp = new KeyboardEvent('keyup', {{ key: '{}', bubbles: true }});
                        el.dispatchEvent(evtUp);
                        return true;
                    }}"#,
                    key, key
                );
                match page.evaluate(script.as_str()).await {
                    Ok(_) => {
                        crate::browser::instrument::auto_wait(page).await;
                        Ok(json!({ "success": true, "key": key }))
                    }
                    Err(e) => Err(format!("Failed to press key: {}", e)),
                }
            }

            BrowserAction::Drag {
                selector,
                target_selector,
                delta_x,
                delta_y,
            } => {
                let script = if let Some(target) = target_selector {
                    format!(
                        r#"() => {{
                            const src = document.querySelector('{}');
                            const dst = document.querySelector('{}');
                            if (!src || !dst) return {{ error: 'Element not found' }};
                            const srcRect = src.getBoundingClientRect();
                            const dstRect = dst.getBoundingClientRect();
                            const sx = srcRect.left + srcRect.width / 2;
                            const sy = srcRect.top + srcRect.height / 2;
                            const dx = dstRect.left + dstRect.width / 2;
                            const dy = dstRect.top + dstRect.height / 2;
                            ['mousedown','mousemove','mouseup'].forEach((type,i) => {{
                                const e = new MouseEvent(type, {{
                                    bubbles: true, clientX: i === 0 ? sx : dx, clientY: i === 0 ? sy : dy,
                                    buttons: 1
                                }});
                                (i === 0 ? src : document.body).dispatchEvent(e);
                            }});
                            return {{ success: true, from: {{ x: sx, y: sy }}, to: {{ x: dx, y: dy }} }};
                        }}"#,
                        selector, target
                    )
                } else {
                    let dx = delta_x.unwrap_or(100);
                    let dy = delta_y.unwrap_or(0);
                    format!(
                        r#"() => {{
                            const el = document.querySelector('{}');
                            if (!el) return {{ error: 'Element not found' }};
                            const rect = el.getBoundingClientRect();
                            const sx = rect.left + rect.width / 2;
                            const sy = rect.top + rect.height / 2;
                            const dx = sx + {};
                            const dy = sy + {};
                            ['mousedown','mousemove','mouseup'].forEach((type,i) => {{
                                const e = new MouseEvent(type, {{
                                    bubbles: true, clientX: i === 0 ? sx : dx, clientY: i === 0 ? sy : dy,
                                    buttons: 1
                                }});
                                (i === 0 ? el : document.body).dispatchEvent(e);
                            }});
                            return {{ success: true, from: {{ x: sx, y: sy }}, to: {{ x: dx, y: dy }} }};
                        }}"#,
                        selector, dx, dy
                    )
                };
                match page.evaluate(script.as_str()).await {
                    Ok(result) => {
                        let value = result.value().cloned().unwrap_or(json!(null));
                        Ok(json!({ "success": true, "result": value }))
                    }
                    Err(e) => Err(format!("Failed to drag: {}", e)),
                }
            }

            BrowserAction::Select { selector, text, start, end } => {
                let script = if let Some(text) = text {
                    format!(
                        r#"() => {{
                            const el = document.querySelector('{}');
                            if (!el) return {{ error: 'Element not found' }};
                            if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                                el.focus();
                                const idx = el.value.indexOf('{}');
                                if (idx >= 0) {{
                                    el.setSelectionRange(idx, idx + {});
                                    return {{ success: true, selected: '{}' }};
                                }}
                                return {{ error: 'Text not found in element' }};
                            }}
                            const range = document.createRange();
                            const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
                            let node;
                            while ((node = walker.nextNode())) {{
                                const idx = node.textContent.indexOf('{}');
                                if (idx >= 0) {{
                                    range.setStart(node, idx);
                                    range.setEnd(node, idx + {});
                                    const sel = window.getSelection();
                                    sel.removeAllRanges();
                                    sel.addRange(range);
                                    return {{ success: true, selected: '{}' }};
                                }}
                            }}
                            return {{ error: 'Text not found' }};
                        }}"#,
                        selector,
                        text,
                        text.len(),
                        text,
                        text,
                        text.len(),
                        text
                    )
                } else {
                    let s = start.unwrap_or(0);
                    let e = end.unwrap_or(usize::MAX);
                    format!(
                        r#"() => {{
                            const el = document.querySelector('{}');
                            if (!el) return {{ error: 'Element not found' }};
                            if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                                el.focus();
                                const len = el.value.length;
                                const start = Math.min({}, len);
                                const end = Math.min({}, len);
                                el.setSelectionRange(start, end);
                                return {{ success: true, start, end, selected: el.value.substring(start, end) }};
                            }}
                            return {{ error: 'Selection by index only supported for input/textarea' }};
                        }}"#,
                        selector, s, e
                    )
                };
                match page.evaluate(script.as_str()).await {
                    Ok(result) => {
                        let value = result.value().cloned().unwrap_or(json!(null));
                        Ok(json!({ "success": true, "result": value }))
                    }
                    Err(e) => Err(format!("Failed to select: {}", e)),
                }
            }

            BrowserAction::UploadFiles { selector, files } => {
                use chromiumoxide::cdp::browser_protocol::dom::{
                    GetDocumentParams, QuerySelectorParams, SetFileInputFilesParams,
                };
                let doc = page
                    .execute(GetDocumentParams::default())
                    .await
                    .map_err(|e| format!("Failed to get document: {}", e))?;
                let root_id = doc.result.root.node_id;
                let query = page
                    .execute(QuerySelectorParams::new(root_id, &selector))
                    .await
                    .map_err(|e| format!("Failed to query selector: {}", e))?;
                let node_id = query.result.node_id;
                let mut params = SetFileInputFilesParams::new(files);
                params.node_id = Some(node_id);
                match page.execute(params).await {
                    Ok(_) => Ok(json!({
                        "success": true,
                        "selector": selector,
                    })),
                    Err(e) => Err(format!("Failed to set file input files: {}", e)),
                }
            }

            BrowserAction::HandleDialog { action, text } => {
                use chromiumoxide::cdp::browser_protocol::page::HandleJavaScriptDialogParams;
                let accept = action == "accept";
                let mut params = HandleJavaScriptDialogParams::new(accept);
                params.prompt_text = text;
                match page.execute(params).await {
                    Ok(_) => Ok(json!({ "success": true, "action": action })),
                    Err(e) => {
                        Err(format!("Failed to handle dialog (no dialog may be open): {}", e))
                    }
                }
            }

            BrowserAction::SetDownloadBehavior { behavior, download_path } => {
                let browser_ref =
                    browser.ok_or("Download behavior requires a browser session".to_string())?;
                use chromiumoxide::cdp::browser_protocol::browser::{
                    SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
                };
                let behavior_enum = match behavior.as_str() {
                    "allow" => SetDownloadBehaviorBehavior::Allow,
                    "deny" => SetDownloadBehaviorBehavior::Deny,
                    "allowAndName" => SetDownloadBehaviorBehavior::AllowAndName,
                    _ => SetDownloadBehaviorBehavior::Default,
                };
                let mut params = SetDownloadBehaviorParams::new(behavior_enum);
                params.download_path = download_path;
                match browser_ref.execute(params).await {
                    Ok(_) => Ok(json!({ "success": true, "behavior": behavior })),
                    Err(e) => Err(format!("Failed to set download behavior: {}", e)),
                }
            }

            BrowserAction::ListTabs => Err("Tab management requires pool-based mode".to_string()),
            BrowserAction::SwitchTab { .. } => {
                Err("Tab management requires pool-based mode".to_string())
            }
            BrowserAction::CloseTab { .. } => {
                Err("Tab management requires pool-based mode".to_string())
            }
        }
    }

    /// Active screencast sessions keyed by page target id.
    #[cfg(feature = "browser")]
    fn screencast_sessions() -> &'static tokio::sync::Mutex<
        std::collections::HashMap<String, ScreencastSession>,
    > {
        static SESSIONS: std::sync::OnceLock<
            tokio::sync::Mutex<std::collections::HashMap<String, ScreencastSession>>,
        > = std::sync::OnceLock::new();
        SESSIONS.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()))
    }

    /// Start a screencast: JPEG frames are written to
    /// `~/.syscity/artifacts/screencast-<timestamp>/frame-*.jpg`.
    #[cfg(feature = "browser")]
    async fn screencast_start(
        page: &chromiumoxide::Page,
        quality: Option<u32>,
        every_nth_frame: Option<u32>,
    ) -> Result<serde_json::Value, String> {
        use chromiumoxide::cdp::browser_protocol::page::{
            EventScreencastFrame, StartScreencastFormat, StartScreencastParams,
        };
        use futures::StreamExt;

        let key = page.target_id().as_ref().to_string();
        let mut sessions = Self::screencast_sessions().lock().await;
        if sessions.contains_key(&key) {
            return Err("Screencast already active for this page".to_string());
        }

        let dir = crate::dirs::syscity_dir().join("artifacts").join(format!(
            "screencast-{}",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        ));
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| format!("Failed to create screencast dir: {}", e))?;

        // Subscribe before starting so no frames are missed.
        let mut events = page
            .event_listener::<EventScreencastFrame>()
            .await
            .map_err(|e| format!("Failed to subscribe screencast events: {}", e))?;

        let mut params = StartScreencastParams {
            format: Some(StartScreencastFormat::Jpeg),
            quality: Some(i64::from(quality.unwrap_or(80).min(100))),
            ..Default::default()
        };
        params.every_nth_frame = every_nth_frame.map(|n| i64::from(n.max(1)));
        page.execute(params)
            .await
            .map_err(|e| format!("Failed to start screencast: {}", e))?;

        let frames = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let frames_task = frames.clone();
        let page_task = page.clone();
        let dir_task = dir.clone();
        let task = tokio::spawn(async move {
            while let Some(ev) = events.next().await {
                // Ack each frame so Chrome keeps delivering.
                if let Err(e) = page_task
                    .execute(
                        chromiumoxide::cdp::browser_protocol::page::ScreencastFrameAckParams::new(
                            ev.session_id,
                        ),
                    )
                    .await
                {
                    warn!("screencast ack failed, stopping collector: {}", e);
                    break;
                }
                let idx = frames_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(AsRef::<str>::as_ref(&ev.data))
                    .unwrap_or_default();
                let path = dir_task.join(format!("frame-{idx:05}.jpg"));
                if let Err(e) = tokio::fs::write(&path, decoded).await {
                    warn!("Failed to write screencast frame: {}", e);
                }
            }
        });

        sessions.insert(
            key,
            ScreencastSession {
                dir: dir.clone(),
                frames,
                task,
            },
        );
        Ok(json!({
            "success": true,
            "frames_dir": dir,
            "note": "Frames are saved as frame-*.jpg. Call screencast_stop to finish."
        }))
    }

    /// Stop the active screencast for this page.
    #[cfg(feature = "browser")]
    async fn screencast_stop(page: &chromiumoxide::Page) -> Result<serde_json::Value, String> {
        use chromiumoxide::cdp::browser_protocol::page::StopScreencastParams;

        let key = page.target_id().as_ref().to_string();
        let session = Self::screencast_sessions().lock().await.remove(&key);
        let Some(session) = session else {
            return Err("No active screencast for this page".to_string());
        };
        if let Err(e) = page.execute(StopScreencastParams {}).await {
            warn!("stopScreencast command failed: {}", e);
        }
        session.task.abort();
        let frames = session.frames.load(std::sync::atomic::Ordering::SeqCst);
        Ok(json!({
            "success": true,
            "frames_dir": session.dir,
            "frames": frames
        }))
    }

    /// Build response from action results
    #[cfg(feature = "browser")]
    fn build_result(
        results: Vec<Result<serde_json::Value, String>>,
        screenshot_data: Option<String>,
    ) -> ToolExecutionResult {
        let success = results.iter().all(|r| r.is_ok());
        let output = serde_json::to_string_pretty(&results)
            .unwrap_or_else(|_| "Failed to serialize results".to_string());

        let data = if let Some(screenshot) = screenshot_data {
            json!({
                "screenshot_base64": screenshot,
                "results": results
            })
        } else {
            json!({ "results": results })
        };

        if success {
            ToolExecutionResult::success(output).with_data(data)
        } else {
            let errors: Vec<String> = results
                .iter()
                .filter_map(|r| r.as_ref().err().cloned())
                .collect();
            let error_msg = if errors.is_empty() {
                "One or more browser actions failed".to_string()
            } else {
                format!("Browser action errors: {}", errors.join("; "))
            };
            ToolExecutionResult::error(error_msg).with_data(data)
        }
    }

    /// Execute browser actions via pool (persistent session)
    #[cfg(feature = "browser")]
    async fn execute_actions_pool(
        &self,
        actions: Vec<BrowserAction>,
        pool: &std::sync::Arc<crate::browser::BrowserPool>,
    ) -> crate::Result<ToolExecutionResult> {
        let instance = pool.get_or_create(&self.profile).await?;
        let mut current_handle = instance.new_page("about:blank").await?;
        if let Err(e) =
            crate::browser::instrument::ensure_instrumented(&current_handle.page).await
        {
            warn!("Failed to instrument pooled page: {}", e);
        }
        if let Err(e) = crate::browser::network_log::start_capture(&current_handle.page).await {
            warn!("Failed to start network capture on pooled page: {}", e);
        }

        let mut results = Vec::new();
        let mut screenshot_data = None;

        for action in actions {
            debug!("Executing browser action (pool): {:?}", action);
            let result = match action {
                BrowserAction::ListTabs => {
                    let tabs = instance.list_pages().await;
                    let tabs_json: Vec<Value> = tabs
                        .into_iter()
                        .map(
                            |(id, title, url)| json!({"target_id": id, "title": title, "url": url}),
                        )
                        .collect();
                    Ok(json!({
                        "success": true,
                        "tabs": tabs_json,
                        "count": tabs_json.len()
                    }))
                }
                BrowserAction::SwitchTab { index, title } => {
                    let tabs = instance.list_pages().await;
                    let target_id = if let Some(idx) = index {
                        tabs.get(idx).map(|(id, _, _)| id.clone())
                    } else if let Some(ref t) = title {
                        instance.find_page_by_title(t).await
                    } else {
                        None
                    };

                    match target_id {
                        Some(id) => match instance.switch_page(&id).await {
                            Ok(true) => {
                                if let Some(handle) = instance.get_page(&id).await {
                                    current_handle = handle;
                                }
                                Ok(json!({"success": true, "target_id": id}))
                            }
                            Ok(false) => Err("Failed to switch tab: page not found".to_string()),
                            Err(e) => Err(format!("Failed to switch tab: {}", e)),
                        },
                        None => Err("Tab not found".to_string()),
                    }
                }
                BrowserAction::CloseTab { index, title } => {
                    let tabs = instance.list_pages().await;
                    let target_id = if let Some(idx) = index {
                        tabs.get(idx).map(|(id, _, _)| id.clone())
                    } else if let Some(ref t) = title {
                        instance.find_page_by_title(t).await
                    } else {
                        None
                    };

                    match target_id {
                        Some(id) => match instance.close_page(&id).await {
                            Ok(true) => Ok(json!({"success": true, "target_id": id})),
                            Ok(false) => Err("Failed to close tab: page not found".to_string()),
                            Err(e) => Err(format!("Failed to close tab: {}", e)),
                        },
                        None => Err("Tab not found".to_string()),
                    }
                }
                other => {
                    Self::execute_single_action(
                        other,
                        &current_handle.page,
                        Some(instance.browser.as_ref()),
                        &mut screenshot_data,
                    )
                    .await
                }
            };
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
        use std::sync::Arc;

        use chromiumoxide::browser::{Browser, BrowserConfig};
        use futures::StreamExt;

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
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: format!("Browser configuration failed: {}", e),
                cause: None,
            })?;

        let (browser, mut handler) = Browser::launch(config).await.map_err(|e| {
            crate::error::SyscityError::ExternalService {
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
            crate::error::SyscityError::ExternalService {
                source: "Failed to create browser page".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

        if let Err(e) = crate::browser::instrument::ensure_instrumented(&page).await {
            warn!("Failed to instrument page: {}", e);
        }
        if let Err(e) = crate::browser::network_log::start_capture(&page).await {
            warn!("Failed to start network capture: {}", e);
        }

        let mut results = Vec::new();
        let mut screenshot_data = None;

        for action in actions {
            debug!("Executing browser action (legacy): {:?}", action);
            let result = Self::execute_single_action(
                action,
                &page,
                Some(browser.as_ref()),
                &mut screenshot_data,
            )
            .await;
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
        "Automate web browser interactions. Navigate to URLs, click elements, fill forms, take \
         screenshots, extract content, and execute JavaScript. Clicking/typing automatically waits \
         for the page to settle (navigation + network idle). Also captures network traffic \
         (fetch/XHR with response bodies via GetNetworkLog), console messages and uncaught \
         exceptions (GetConsoleMessages), and can record screencasts as JPEG frame sequences \
         (ScreencastStart/ScreencastStop). Use this tool when the user asks to open a webpage, \
         browse the web, take a website screenshot, debug a page's network/console activity, or \
         automate browser actions (打开网页/浏览/网页截图/网页调试). Requires Chrome/Chromium to be \
         installed."
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
                                    "GetNetworkLog": {
                                        "type": "object",
                                        "properties": {
                                            "url": { "type": "string", "description": "Substring filter on request URL" },
                                            "method": { "type": "string", "description": "HTTP method filter (GET, POST, ...)" },
                                            "resource_type": { "type": "string", "description": "Resource type filter (document, xhr, fetch, script, img, stylesheet, ...)" },
                                            "min_status": { "type": "integer", "description": "Minimum HTTP status (inclusive)" },
                                            "max_status": { "type": "integer", "description": "Maximum HTTP status (inclusive)" },
                                            "include_body": { "type": "boolean", "description": "Include truncated response bodies (default: true)" },
                                            "limit": { "type": "integer", "description": "Max entries to return (default: 50)" },
                                            "offset": { "type": "integer", "description": "Entries to skip for pagination (default: 0)" }
                                        }
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "FillForm": {
                                        "type": "object",
                                        "properties": {
                                            "fields": {
                                                "type": "array",
                                                "items": {
                                                    "type": "object",
                                                    "properties": {
                                                        "selector": { "type": "string", "description": "CSS selector of the input" },
                                                        "value": { "type": "string", "description": "Value to type" },
                                                        "clear": { "type": "boolean", "description": "Clear first (default: true)" }
                                                    },
                                                    "required": ["selector", "value"]
                                                },
                                                "description": "Fields to fill"
                                            }
                                        },
                                        "required": ["fields"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "Hover": {
                                        "type": "object",
                                        "properties": {
                                            "selector": { "type": "string", "description": "CSS selector to hover" }
                                        },
                                        "required": ["selector"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "ClickAt": {
                                        "type": "object",
                                        "properties": {
                                            "x": { "type": "number", "description": "Viewport x coordinate" },
                                            "y": { "type": "number", "description": "Viewport y coordinate" }
                                        },
                                        "required": ["x", "y"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "EmulateNetwork": {
                                        "type": "object",
                                        "properties": {
                                            "latency_ms": { "type": "number", "description": "Extra latency in ms (default: 0)" },
                                            "download_bps": { "type": "number", "description": "Download throughput bytes/sec (default: unlimited)" },
                                            "upload_bps": { "type": "number", "description": "Upload throughput bytes/sec (default: unlimited)" },
                                            "offline": { "type": "boolean", "description": "Simulate offline (default: false)" }
                                        }
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "EmulateCpu": {
                                        "type": "object",
                                        "properties": {
                                            "rate": { "type": "number", "description": "CPU slowdown factor (1 = none, 4 = 4x slower)" }
                                        },
                                        "required": ["rate"]
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "GetConsoleMessages": {
                                        "type": "object",
                                        "properties": {
                                            "level": { "type": "string", "enum": ["log", "info", "warn", "error", "debug", "exception"], "description": "Level filter" },
                                            "limit": { "type": "integer", "description": "Max messages to return (default: 100)" }
                                        }
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "ClearCaptures": { "type": "object", "properties": {} }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "ScreencastStart": {
                                        "type": "object",
                                        "properties": {
                                            "quality": { "type": "integer", "description": "JPEG quality 0-100 (default: 80)" },
                                            "every_nth_frame": { "type": "integer", "description": "Save every Nth frame (default: 1)" }
                                        }
                                    }
                                }
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "ScreencastStop": { "type": "object", "properties": {} }
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

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: true,
            risk_level: crate::tools::approval::RiskLevel::High,
            categories: vec!["network".to_string(), "browser".to_string()],
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        mut args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        // Normalize action names to handle LLM-generated PascalCase variant names
        if let Some(actions_val) = args.get_mut("actions") {
            normalize_browser_actions(actions_val);
        }

        let actions: Vec<BrowserAction> = serde_json::from_value(
            args.get("actions").cloned().unwrap_or(json!([])),
        )
        .map_err(|e| {
            crate::error::SyscityError::Validation(format!("Invalid browser actions: {}", e))
        })?;

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

        let net = BrowserAction::GetNetworkLog {
            url: None,
            method: None,
            resource_type: None,
            min_status: None,
            max_status: None,
            include_body: None,
            limit: None,
            offset: None,
        };
        let json = serde_json::to_string(&net).unwrap();
        assert!(json.contains("get_network_log"));

        let net_filtered = BrowserAction::GetNetworkLog {
            url: Some("/api/".to_string()),
            method: Some("POST".to_string()),
            resource_type: Some("xhr".to_string()),
            min_status: Some(200),
            max_status: Some(299),
            include_body: Some(false),
            limit: Some(10),
            offset: Some(5),
        };
        let json = serde_json::to_string(&net_filtered).unwrap();
        assert!(json.contains("/api/"));
        let roundtrip: BrowserAction = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            roundtrip,
            BrowserAction::GetNetworkLog {
                min_status: Some(200),
                limit: Some(10),
                ..
            }
        ));

        let console = BrowserAction::GetConsoleMessages {
            level: Some("error".to_string()),
            limit: Some(20),
        };
        let json = serde_json::to_string(&console).unwrap();
        assert!(json.contains("get_console_messages"));
        assert!(json.contains("error"));

        let clear = BrowserAction::ClearCaptures;
        let json = serde_json::to_string(&clear).unwrap();
        assert!(json.contains("clear_captures"));

        let cast_start = BrowserAction::ScreencastStart {
            quality: Some(70),
            every_nth_frame: Some(2),
        };
        let json = serde_json::to_string(&cast_start).unwrap();
        assert!(json.contains("screencast_start"));
        assert!(json.contains("70"));

        let cast_stop = BrowserAction::ScreencastStop;
        let json = serde_json::to_string(&cast_stop).unwrap();
        assert!(json.contains("screencast_stop"));

        let form = BrowserAction::FillForm {
            fields: vec![
                FormField {
                    selector: "#user".to_string(),
                    value: "alice".to_string(),
                    clear: None,
                },
                FormField {
                    selector: "#pass".to_string(),
                    value: "secret".to_string(),
                    clear: Some(false),
                },
            ],
        };
        let json = serde_json::to_string(&form).unwrap();
        assert!(json.contains("fill_form"));
        assert!(json.contains("#user"));
        let roundtrip: BrowserAction = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            roundtrip,
            BrowserAction::FillForm { ref fields } if fields.len() == 2
        ));

        let hover = BrowserAction::Hover {
            selector: "#menu".to_string(),
        };
        let json = serde_json::to_string(&hover).unwrap();
        assert!(json.contains("hover"));

        let click_at = BrowserAction::ClickAt { x: 100.5, y: 200.0 };
        let json = serde_json::to_string(&click_at).unwrap();
        assert!(json.contains("click_at"));

        let emu_net = BrowserAction::EmulateNetwork {
            latency_ms: Some(200.0),
            download_bps: Some(1_000_000.0),
            upload_bps: None,
            offline: Some(false),
        };
        let json = serde_json::to_string(&emu_net).unwrap();
        assert!(json.contains("emulate_network"));
        assert!(json.contains("200"));

        let emu_cpu = BrowserAction::EmulateCpu { rate: 4.0 };
        let json = serde_json::to_string(&emu_cpu).unwrap();
        assert!(json.contains("emulate_cpu"));

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

        let press = BrowserAction::Press { key: "Enter".to_string() };
        let json = serde_json::to_string(&press).unwrap();
        assert!(json.contains("press"));
        assert!(json.contains("Enter"));

        let drag = BrowserAction::Drag {
            selector: "#item".to_string(),
            target_selector: Some("#dropzone".to_string()),
            delta_x: Some(100),
            delta_y: Some(0),
        };
        let json = serde_json::to_string(&drag).unwrap();
        assert!(json.contains("drag"));
        assert!(json.contains("#item"));

        let select = BrowserAction::Select {
            selector: "#input".to_string(),
            text: Some("hello".to_string()),
            start: None,
            end: None,
        };
        let json = serde_json::to_string(&select).unwrap();
        assert!(json.contains("select"));
        assert!(json.contains("hello"));

        let upload = BrowserAction::UploadFiles {
            selector: "#file".to_string(),
            files: vec!["/tmp/test.txt".to_string()],
        };
        let json = serde_json::to_string(&upload).unwrap();
        assert!(json.contains("upload_files"));
        assert!(json.contains("/tmp/test.txt"));

        let dialog = BrowserAction::HandleDialog {
            action: "accept".to_string(),
            text: Some("ok".to_string()),
        };
        let json = serde_json::to_string(&dialog).unwrap();
        assert!(json.contains("handle_dialog"));
        assert!(json.contains("accept"));

        let download = BrowserAction::SetDownloadBehavior {
            behavior: "allow".to_string(),
            download_path: Some("/tmp".to_string()),
        };
        let json = serde_json::to_string(&download).unwrap();
        assert!(json.contains("set_download_behavior"));
        assert!(json.contains("allow"));

        let list_tabs = BrowserAction::ListTabs;
        let json = serde_json::to_string(&list_tabs).unwrap();
        assert!(json.contains("list_tabs"));

        let switch_tab = BrowserAction::SwitchTab {
            index: Some(1),
            title: Some("Example".to_string()),
        };
        let json = serde_json::to_string(&switch_tab).unwrap();
        assert!(json.contains("switch_tab"));
        assert!(json.contains("Example"));

        let close_tab = BrowserAction::CloseTab { index: Some(0), title: None };
        let json = serde_json::to_string(&close_tab).unwrap();
        assert!(json.contains("close_tab"));
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
        assert!(matches!(snapshot, BrowserAction::Snapshot { max_chars: Some(5000) }));

        let act: BrowserAction = serde_json::from_value(json!({
            "act": { "ref_id": 7, "action": { "click": null } }
        }))
        .unwrap();
        assert!(matches!(
            act,
            BrowserAction::Act {
                ref_id: 7,
                action: crate::browser::ActKind::Click
            }
        ));

        let act_type: BrowserAction = serde_json::from_value(json!({
            "act": { "ref_id": 2, "action": { "type": { "text": "hello" } } }
        }))
        .unwrap();
        assert!(matches!(
            act_type,
            BrowserAction::Act { ref_id: 2, action: crate::browser::ActKind::Type { ref text } } if text == "hello"
        ));

        let press: BrowserAction = serde_json::from_value(json!({
            "press": { "key": "Enter" }
        }))
        .unwrap();
        assert!(matches!(press, BrowserAction::Press { ref key } if key == "Enter"));

        let drag: BrowserAction = serde_json::from_value(json!({
            "drag": { "selector": "#item", "target_selector": "#dropzone", "delta_x": 100, "delta_y": 0 }
        })).unwrap();
        assert!(matches!(drag, BrowserAction::Drag { ref selector, .. } if selector == "#item"));

        let select: BrowserAction = serde_json::from_value(json!({
            "select": { "selector": "#input", "text": "hello", "start": 0, "end": 5 }
        }))
        .unwrap();
        assert!(
            matches!(select, BrowserAction::Select { ref selector, .. } if selector == "#input")
        );

        let upload: BrowserAction = serde_json::from_value(json!({
            "upload_files": { "selector": "#file", "files": ["/tmp/test.txt"] }
        }))
        .unwrap();
        assert!(
            matches!(upload, BrowserAction::UploadFiles { ref selector, .. } if selector == "#file")
        );

        let dialog: BrowserAction = serde_json::from_value(json!({
            "handle_dialog": { "action": "accept", "text": "ok" }
        }))
        .unwrap();
        assert!(
            matches!(dialog, BrowserAction::HandleDialog { ref action, .. } if action == "accept")
        );

        let download: BrowserAction = serde_json::from_value(json!({
            "set_download_behavior": { "behavior": "allow", "download_path": "/tmp" }
        }))
        .unwrap();
        assert!(
            matches!(download, BrowserAction::SetDownloadBehavior { ref behavior, .. } if behavior == "allow")
        );

        let list_tabs: BrowserAction = serde_json::from_value(json!({"list_tabs": null})).unwrap();
        assert!(matches!(list_tabs, BrowserAction::ListTabs));

        let switch_tab: BrowserAction = serde_json::from_value(json!({
            "switch_tab": { "index": 1, "title": "Example" }
        }))
        .unwrap();
        assert!(matches!(switch_tab, BrowserAction::SwitchTab { index: Some(1), .. }));

        let close_tab: BrowserAction = serde_json::from_value(json!({
            "close_tab": { "index": 0 }
        }))
        .unwrap();
        assert!(matches!(close_tab, BrowserAction::CloseTab { index: Some(0), .. }));
    }
}
