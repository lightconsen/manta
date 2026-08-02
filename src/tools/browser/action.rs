//! Browser action type definitions and normalization.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
pub(super) fn normalize_browser_actions(value: &mut Value) {
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
