//! Browser profile configuration
//!
//! Defines browser profiles with varying headless mode, viewport, user agent,
//! and driver type (managed launch vs Chrome MCP connection).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Browser driver type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDriver {
    /// chromiumoxide launches and manages Chrome
    #[default]
    Managed,
    /// Connect to an existing Chrome via CDP
    ChromeMcp { cdp_url: String },
}

/// Browser profile configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProfile {
    /// Profile name (e.g. "default", "headed", "mcp")
    pub name: String,
    /// Run in headless mode
    #[serde(default = "default_headless")]
    pub headless: bool,
    /// Viewport width in pixels
    #[serde(default = "default_viewport_width")]
    pub viewport_width: u32,
    /// Viewport height in pixels
    #[serde(default = "default_viewport_height")]
    pub viewport_height: u32,
    /// Custom user agent (None = use Chrome default)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Chrome user data directory (for persistent sessions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data_dir: Option<PathBuf>,
    /// Path to Chrome/Chromium executable (None = auto-detect)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chrome_path: Option<PathBuf>,
    /// Driver type
    #[serde(default)]
    pub driver: BrowserDriver,
    /// Additional Chrome arguments
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl Default for BrowserProfile {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            headless: default_headless(),
            viewport_width: default_viewport_width(),
            viewport_height: default_viewport_height(),
            user_agent: None,
            user_data_dir: None,
            chrome_path: None,
            driver: BrowserDriver::Managed,
            extra_args: Vec::new(),
        }
    }
}

fn default_headless() -> bool {
    true
}

fn default_viewport_width() -> u32 {
    1280
}

fn default_viewport_height() -> u32 {
    720
}

impl BrowserProfile {
    /// Create a new default profile
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Create a headed (non-headless) profile
    pub fn headed(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            headless: false,
            ..Default::default()
        }
    }

    /// Create a Chrome MCP profile
    pub fn chrome_mcp(name: impl Into<String>, cdp_url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            driver: BrowserDriver::ChromeMcp { cdp_url: cdp_url.into() },
            ..Default::default()
        }
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

    /// Set user agent
    pub fn with_user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    /// Set Chrome executable path
    pub fn with_chrome_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.chrome_path = Some(path.into());
        self
    }

    /// Set user data directory
    pub fn with_user_data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_data_dir = Some(path.into());
        self
    }

    /// Get the CDP URL for Chrome MCP driver
    pub fn cdp_url(&self) -> Option<&str> {
        match &self.driver {
            BrowserDriver::ChromeMcp { cdp_url } => Some(cdp_url),
            _ => None,
        }
    }
}

/// Pool configuration for browser instance management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserPoolConfig {
    /// Maximum idle time before evicting an instance (seconds)
    #[serde(default = "default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// Cleanup interval for idle instances (seconds)
    #[serde(default = "default_cleanup_interval_secs")]
    pub cleanup_interval_secs: u64,
    /// Default profile to use when none is specified
    #[serde(default = "default_profile_name")]
    pub default_profile: String,
}

impl Default for BrowserPoolConfig {
    fn default() -> Self {
        Self {
            idle_timeout_secs: default_idle_timeout_secs(),
            cleanup_interval_secs: default_cleanup_interval_secs(),
            default_profile: default_profile_name(),
        }
    }
}

fn default_idle_timeout_secs() -> u64 {
    300 // 5 minutes
}

fn default_cleanup_interval_secs() -> u64 {
    60 // 1 minute
}

fn default_profile_name() -> String {
    "default".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_profile_default() {
        let p = BrowserProfile::default();
        assert_eq!(p.name, "default");
        assert!(p.headless);
        assert_eq!(p.viewport_width, 1280);
        assert_eq!(p.viewport_height, 720);
        assert_eq!(p.driver, BrowserDriver::Managed);
    }

    #[test]
    fn test_browser_profile_headed() {
        let p = BrowserProfile::headed("headed");
        assert_eq!(p.name, "headed");
        assert!(!p.headless);
    }

    #[test]
    fn test_browser_profile_chrome_mcp() {
        let p = BrowserProfile::chrome_mcp("mcp", "http://127.0.0.1:9222");
        assert_eq!(p.name, "mcp");
        assert_eq!(p.cdp_url(), Some("http://127.0.0.1:9222"));
    }

    #[test]
    fn test_browser_profile_builder() {
        let p = BrowserProfile::new("custom")
            .with_viewport(1920, 1080)
            .with_headless(false)
            .with_user_agent("TestAgent/1.0");
        assert_eq!(p.viewport_width, 1920);
        assert_eq!(p.viewport_height, 1080);
        assert!(!p.headless);
        assert_eq!(p.user_agent, Some("TestAgent/1.0".to_string()));
    }

    #[test]
    fn test_browser_profile_serde() {
        let p = BrowserProfile::new("test")
            .with_viewport(800, 600)
            .with_chrome_path("/usr/bin/chrome");
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"name\":\"test\""));
        assert!(json.contains("800"));

        let de: BrowserProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(de.name, "test");
        assert_eq!(de.viewport_width, 800);
    }

    #[test]
    fn test_browser_profile_serde_chrome_mcp() {
        let p = BrowserProfile::chrome_mcp("mcp", "http://localhost:9222");
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("chrome_mcp"));
        assert!(json.contains("http://localhost:9222"));

        let de: BrowserProfile = serde_json::from_str(&json).unwrap();
        assert!(matches!(de.driver, BrowserDriver::ChromeMcp { .. }));
    }

    #[test]
    fn test_pool_config_default() {
        let c = BrowserPoolConfig::default();
        assert_eq!(c.idle_timeout_secs, 300);
        assert_eq!(c.cleanup_interval_secs, 60);
        assert_eq!(c.default_profile, "default");
    }
}
