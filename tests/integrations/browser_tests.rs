//! Browser integration tests
//!
//! These tests require the `browser` feature and may require Chrome/Chromium
//! to be installed for some tests.

#![cfg(feature = "browser")]

use manta::browser::{
    ActKind, BrowserPool, BrowserPoolConfig, BrowserProfile, NavigationPolicy,
    assert_navigation_allowed,
};

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

    // Default profile should exist
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let profiles = pool.status().await;
        assert!(profiles.is_empty()); // No instances created yet
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
