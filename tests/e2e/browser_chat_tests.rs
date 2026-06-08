//! Browser tool E2E tests
//!
//! Requires:
//! - `browser` feature enabled
//! - LLM provider configured (SYSCITY_TEST_PROVIDER_KEY)
//! - Chrome/Chromium installed on the system

use super::*;

/// Check if Chrome/Chromium is available on the system
fn chrome_available() -> bool {
    // Check common executable names
    for cmd in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ] {
        if std::process::Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    // Also check macOS default location
    std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome").exists()
        || std::path::Path::new("/Applications/Chromium.app/Contents/MacOS/Chromium").exists()
}

fn skip_if_no_chrome() {
    if !chrome_available() {
        eprintln!(
            "Skipping browser E2E test: Chrome/Chromium not found. \
             Install Chrome or set CHROME_PATH env var."
        );
    }
}

#[tokio::test]
#[serial]
#[cfg(feature = "browser")]
async fn tool_browser_navigate_invoked_via_chat() {
    skip_if_no_chrome();
    if !chrome_available() {
        return;
    }
    let _results = run_tool_chat_test(
        40120,
        "Use the browser tool to navigate to https://example.com and tell me the page title.",
        "browser",
    )
    .await;
}

#[tokio::test]
#[serial]
#[cfg(feature = "browser")]
async fn tool_browser_snapshot_invoked_via_chat() {
    skip_if_no_chrome();
    if !chrome_available() {
        return;
    }
    let _results = run_tool_chat_test(
        40121,
        "Use the browser tool to navigate to https://example.com, then take a snapshot and tell me what interactive elements are on the page.",
        "browser",
    )
    .await;
}

#[tokio::test]
#[serial]
#[cfg(feature = "browser")]
async fn tool_browser_screenshot_invoked_via_chat() {
    skip_if_no_chrome();
    if !chrome_available() {
        return;
    }
    let _results = run_tool_chat_test(
        40122,
        "Use the browser tool to navigate to https://example.com and take a screenshot, then tell me if the screenshot was saved.",
        "browser",
    )
    .await;
}

#[tokio::test]
#[serial]
#[cfg(feature = "browser")]
async fn tool_browser_pdf_invoked_via_chat() {
    skip_if_no_chrome();
    if !chrome_available() {
        return;
    }
    let _results = run_tool_chat_test(
        40123,
        "Use the browser tool to navigate to https://example.com and save the page as a PDF.",
        "browser",
    )
    .await;
}

#[tokio::test]
#[serial]
#[cfg(feature = "browser")]
async fn tool_browser_click_and_type_invoked_via_chat() {
    skip_if_no_chrome();
    if !chrome_available() {
        return;
    }
    let _results = run_tool_chat_test(
        40124,
        "Use the browser tool to navigate to https://example.com, click the 'More information...' link, and tell me what page you land on.",
        "browser",
    )
    .await;
}
