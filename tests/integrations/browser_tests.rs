//! Browser integration tests
//!
//! These tests require the `browser` feature and may require Chrome/Chromium
//! to be installed for some tests.
//!
//! Note: chromiumoxide 0.7 may not be compatible with Chrome 128+ due to
//! CDP protocol changes. Tests detect this and skip gracefully.

#![cfg(feature = "browser")]

use syscity::browser::{
    assert_navigation_allowed, ActKind, BrowserPool, BrowserPoolConfig, BrowserProfile,
    NavigationPolicy,
};
use syscity::tools::browser::BrowserTool;
use syscity::tools::{Tool, ToolContext};
use serde_json::json;
use serial_test::serial;

/// Check if Chrome/Chromium is available on the system
fn chrome_available() -> bool {
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
    std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome").exists()
        || std::path::Path::new("/Applications/Chromium.app/Contents/MacOS/Chromium").exists()
}

/// Detect whether Chrome version is compatible with chromiumoxide 0.9.
/// Very new Chrome versions (200+) may send CDP messages that future
/// chromiumoxide versions cannot deserialize.
fn chrome_compatible() -> bool {
    if !chrome_available() {
        return false;
    }
    let commands: Vec<String> = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain(std::iter::once(
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
    ))
    .chain(std::iter::once(
        "/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
    ))
    .collect();

    for cmd in &commands {
        if let Ok(output) = std::process::Command::new(cmd).arg("--version").output() {
            if let Ok(version_str) = String::from_utf8(output.stdout) {
                let parts: Vec<&str> = version_str.split_whitespace().collect();
                for part in &parts {
                    if let Some(dot_idx) = part.find('.') {
                        if let Ok(major) = part[..dot_idx].parse::<u32>() {
                            if major > 200 {
                                eprintln!(
                                    "Skipping: Chrome {} may be too new for chromiumoxide 0.9. \
                                     Consider upgrading chromiumoxide if tests fail.",
                                    major
                                );
                                return false;
                            }
                            return true;
                        }
                    }
                }
            }
        }
    }
    true
}

fn skip_if_incompatible() {
    if !chrome_available() {
        eprintln!("Skipping: Chrome/Chromium not found.");
    } else if !chrome_compatible() {
        // chrome_compatible() already prints the reason
    }
}

#[test]
fn test_browser_navigate_blocks_private_ip() {
    let policy = NavigationPolicy::restrictive();

    assert!(assert_navigation_allowed("http://127.0.0.1/", &policy).is_err());
    assert!(assert_navigation_allowed("http://10.0.0.1/", &policy).is_err());
    assert!(assert_navigation_allowed("http://192.168.1.1/", &policy).is_err());
    assert!(assert_navigation_allowed("http://172.16.0.1/", &policy).is_err());
    assert!(assert_navigation_allowed("http://[::1]/", &policy).is_err());
    assert!(assert_navigation_allowed("http://localhost/", &policy).is_err());
    assert!(assert_navigation_allowed("https://example.com/", &policy).is_ok());
}

#[test]
fn test_browser_profile_serde_roundtrip() {
    let profile = BrowserProfile::new("test")
        .with_viewport(1920, 1080)
        .with_headless(false)
        .with_user_agent("TestAgent/1.0");

    let json = serde_json::to_string(&profile).unwrap();
    let de: BrowserProfile = serde_json::from_str(&json).unwrap();

    assert_eq!(de.name, "test");
    assert_eq!(de.viewport_width, 1920);
    assert_eq!(de.viewport_height, 1080);
    assert!(!de.headless);
    assert_eq!(de.user_agent, Some("TestAgent/1.0".to_string()));
}

#[test]
fn test_browser_pool_lifecycle() {
    let config = BrowserPoolConfig::default();
    let pool = BrowserPool::new(config);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let profiles = pool.status().await;
        assert!(profiles.is_empty());
    });
}

#[test]
fn test_browser_pool_register_and_status() {
    let config = BrowserPoolConfig::default();
    let pool = BrowserPool::new(config);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let profile = BrowserProfile::headed("test-headed");
        pool.register_profile(profile).await;

        let status = pool.status().await;
        assert!(status.is_empty());
    });
}

#[test]
fn test_browser_pool_with_profiles() {
    let profiles = vec![
        BrowserProfile::new("default"),
        BrowserProfile::headed("headed"),
    ];
    let config = BrowserPoolConfig::default();
    let pool = BrowserPool::with_profiles(config, profiles);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let status = pool.status().await;
        assert!(status.is_empty());
    });
}

#[test]
fn test_act_kind_serde() {
    let click = ActKind::Click;
    let json = serde_json::to_string(&click).unwrap();
    assert!(json.contains("click"));

    let type_action = ActKind::Type { text: "hello".to_string() };
    let json = serde_json::to_string(&type_action).unwrap();
    assert!(json.contains("type"));
    assert!(json.contains("hello"));

    let fill = ActKind::Fill { text: "world".to_string() };
    let json = serde_json::to_string(&fill).unwrap();
    assert!(json.contains("fill"));

    let hover = ActKind::Hover;
    let json = serde_json::to_string(&hover).unwrap();
    assert!(json.contains("hover"));
}

#[test]
fn test_navigation_guard_allowlist() {
    let policy = NavigationPolicy {
        allow_private: false,
        allowed_hostnames: vec!["example.com".to_string()],
        blocked_hostnames: Vec::new(),
    };

    assert!(assert_navigation_allowed("https://example.com/", &policy).is_ok());
    assert!(assert_navigation_allowed("https://google.com/", &policy).is_err());
}

#[test]
fn test_navigation_guard_schemes() {
    let policy = NavigationPolicy::restrictive();

    assert!(assert_navigation_allowed("http://example.com/", &policy).is_ok());
    assert!(assert_navigation_allowed("https://example.com/", &policy).is_ok());
    assert!(assert_navigation_allowed("file:///etc/passwd", &policy).is_err());
    assert!(assert_navigation_allowed("ftp://example.com/", &policy).is_err());
}

// ── Direct Browser Tool Integration Tests (require compatible Chrome) ───────────

#[tokio::test]
#[serial]
async fn test_browser_click() {
    skip_if_incompatible();
    if !chrome_compatible() {
        return;
    }

    let tool = BrowserTool::new();
    let ctx = ToolContext::default();
    let args = json!({
        "actions": [
            { "navigate": { "url": "data:text/html,<html><body><button id='btn' onclick=\"document.body.innerText='clicked'\">Click</button></body></html>" } },
            { "click": { "selector": "#btn" } },
            { "get_text": {} }
        ]
    });

    let result = tool.execute(args, &ctx).await.unwrap();
    assert!(result.success, "browser click failed: {:?}", result.error);
    let data = result.data.expect("expected data");
    let results = data
        .get("results")
        .expect("expected results")
        .as_array()
        .expect("expected array");
    assert_eq!(results.len(), 3);
    let ok_val = results[2].get("Ok").expect("expected Ok");
    let text = ok_val.get("text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(text.contains("clicked"), "expected 'clicked' in page text, got: {}", text);
}

#[tokio::test]
#[serial]
async fn test_browser_type() {
    skip_if_incompatible();
    if !chrome_compatible() {
        return;
    }

    let tool = BrowserTool::new();
    let ctx = ToolContext::default();
    let args = json!({
        "actions": [
            { "navigate": { "url": "data:text/html,<html><body><input id='input' type='text'><div id='result'></div><script>document.getElementById('input').addEventListener('input', function(e) { document.getElementById('result').innerText = e.target.value; });</script></body></html>" } },
            { "type": { "selector": "#input", "text": "hello", "clear": true } },
            { "get_text": { "selector": "#result" } }
        ]
    });

    let result = tool.execute(args, &ctx).await.unwrap();
    assert!(result.success, "browser type failed: {:?}", result.error);
    let data = result.data.expect("expected data");
    let results = data
        .get("results")
        .expect("expected results")
        .as_array()
        .expect("expected array");
    assert_eq!(results.len(), 3);
    let ok_val = results[2].get("Ok").expect("expected Ok");
    let text = ok_val.get("text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(text.contains("hello"), "expected 'hello' in result, got: {}", text);
}

#[tokio::test]
#[serial]
async fn test_browser_scroll() {
    skip_if_incompatible();
    if !chrome_compatible() {
        return;
    }

    let tool = BrowserTool::new();
    let ctx = ToolContext::default();
    let args = json!({
        "actions": [
            { "navigate": { "url": "data:text/html,<html><body><div style='height:3000px'></div><div id='bottom'>Bottom</div></body></html>" } },
            { "scroll": { "direction": "down", "amount": 1000 } },
            { "execute_script": { "script": "return window.scrollY;" } }
        ]
    });

    let result = tool.execute(args, &ctx).await.unwrap();
    assert!(result.success, "browser scroll failed: {:?}", result.error);
    let data = result.data.expect("expected data");
    let results = data
        .get("results")
        .expect("expected results")
        .as_array()
        .expect("expected array");
    assert_eq!(results.len(), 3);
    let ok_val = results[2].get("Ok").expect("expected Ok");
    let scroll_y = ok_val.get("result").and_then(|v| v.as_f64()).unwrap_or(0.0);
    assert!(scroll_y > 0.0, "expected scrollY > 0, got: {}", scroll_y);
}

#[tokio::test]
#[serial]
async fn test_browser_press() {
    skip_if_incompatible();
    if !chrome_compatible() {
        return;
    }

    let tool = BrowserTool::new();
    let ctx = ToolContext::default();
    let args = json!({
        "actions": [
            { "navigate": { "url": "data:text/html,<html><body><input id='input' type='text' onkeydown=\"document.getElementById('result').innerText='pressed:'+event.key\"><div id='result'></div></body></html>" } },
            { "click": { "selector": "#input" } },
            { "press": { "key": "a" } },
            { "get_text": { "selector": "#result" } }
        ]
    });

    let result = tool.execute(args, &ctx).await.unwrap();
    assert!(result.success, "browser press failed: {:?}", result.error);
    let data = result.data.expect("expected data");
    let results = data
        .get("results")
        .expect("expected results")
        .as_array()
        .expect("expected array");
    assert_eq!(results.len(), 4);
    let ok_val = results[3].get("Ok").expect("expected Ok");
    let text = ok_val.get("text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(text.contains("pressed:a"), "expected 'pressed:a' in result, got: {}", text);
}
