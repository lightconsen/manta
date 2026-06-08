//! Computer adapter — cross-platform unified desktop interface.
//!
//! This layer sits **above** `CapabilitySet` + `ToolRegistry` and provides
//! a platform-agnostic API for screenshots, UI-tree reading, and desktop
//! actions.  The LLM still interacts with individual `Tool`s via
//! `ToolRegistry`; `ComputerAdapter` is consumed by the higher-level
//! `GoalPlanner` and `ComputerUseLoop`.
//!
//! Architecture:
//! ```text
//! Agent / Planner
//!       |
//! ComputerAdapter  ←── 统一接口（你在这里）
//!       |
//! ToolRegistry ──→ CapabilitySet ──→ xdotool / SendKeys / AXUIElement
//! ```

use crate::tools::ToolRegistry;
use std::sync::Arc;
use std::time::Duration;

pub mod types;
pub mod system;
pub mod verification;
pub mod reflection;
pub mod use_loop;
pub mod rollback;
pub mod headless;
pub mod fs_watch;
pub mod network;
pub mod log_aggregator;
pub mod audio;
pub mod screen_recorder;
pub mod screenshot_encoder;
pub mod sensitive_ui;
pub mod remote_control;
#[cfg(feature = "vision")]
pub mod vision;

// Platform adapters
#[cfg(target_os = "macos")]
pub mod platform_macos;
#[cfg(target_os = "windows")]
pub mod platform_windows;
#[cfg(target_os = "linux")]
pub mod platform_linux;

pub use remote_control::{RemoteControlAdapter, RemoteControlConfig, RemoteProtocol};
pub use types::*;
pub use verification::{VerificationConfig, VerificationCriteria, VerificationEngine};
pub use use_loop::{ComputerUseLoop, LoopConfig, LoopDecision, LoopResult, LoopState, StepRecord};
pub use rollback::{RollbackManager, Snapshot};
pub use headless::{HeadlessComputerAdapter, VirtualDisplay};
pub use fs_watch::{FileChangeEvent, FileChangeKind, FileWatchResult, FileWatcher};
pub use network::{FirewallRule, NetworkInspector, PingResult, PortEntry, TcpConnectResult};
pub use log_aggregator::{AlertAction, AlertEvent, LogAggregator, LogAlertRule, LogEntry, LogLevel, LogSource};
pub use audio::{AudioCapture, AudioSegment, AudioSource, DetectedAudioEvent};
pub use screen_recorder::{RecorderConfig, Rect as RecorderRect, ScreenRecorder, VideoFrame};

/// Unified error type for computer operations.
#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    #[error("Platform not supported: {0}")]
    UnsupportedPlatform(String),
    #[error("No GUI/display server available")]
    NoDisplay,
    #[error("Accessibility permission not granted")]
    AccessibilityDenied,
    #[error("Tool execution failed: {0}")]
    ToolFailed(String),
    #[error("Element not found: {0}")]
    ElementNotFound(String),
    #[error("Timeout waiting for condition")]
    Timeout,
    #[error("Screenshot failed: {0}")]
    ScreenshotFailed(String),
    #[error("Adapter not initialized")]
    NotInitialized,
    #[error("Process not found: {0}")]
    ProcessNotFound(String),
    #[error("Failed to kill process: {0}")]
    KillFailed(String),
    #[error("Other: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ComputerError>;

/// Cross-platform adapter for desktop perception and action.
///
/// Implementors wrap platform-specific `CapabilitySet` tools (xdotool,
/// SendKeys, AXUIElement, UIAutomation, etc.) and expose a uniform
/// interface to the Agent layer.
#[async_trait::async_trait]
pub trait ComputerAdapter: Send + Sync {
    /// Capture a screenshot of the full screen or a sub-region.
    async fn screenshot(&self, region: Option<Rect>) -> Result<Screenshot>;

    /// Read the accessibility UI tree of the active application.
    async fn read_ui_tree(&self, app: Option<&str>) -> Result<Vec<UiElement>>;

    /// Execute a desktop action.
    async fn execute(&self, action: DesktopAction) -> Result<ActionResult>;

    /// Wait until a condition becomes true or timeout expires.
    async fn wait_for(
        &self,
        condition: WaitCondition,
        timeout: Duration,
    ) -> Result<bool>;

    /// Convenience: click at a logical coordinate.
    async fn click_at(
        &self,
        point: Point,
        button: MouseButton,
    ) -> Result<ActionResult> {
        self.execute(DesktopAction::Click {
            target: ClickTarget::Coordinate(point),
            button,
        })
        .await
    }

    /// Convenience: type text.
    async fn type_text(&self,
        text: &str,
    ) -> Result<ActionResult> {
        self.execute(DesktopAction::Type {
            text: text.to_string(),
        })
        .await
    }

    /// Convenience: press a key combination.
    async fn key_press(&self,
        keys: Vec<String>,
    ) -> Result<ActionResult> {
        self.execute(DesktopAction::KeyPress { keys }).await
    }

    /// Convenience: get clipboard content.
    async fn clipboard_get(&self) -> Result<String> {
        let result = self.execute(DesktopAction::ClipboardGet).await?;
        if result.success {
            Ok(result.message)
        } else {
            Err(ComputerError::ToolFailed(result.message))
        }
    }

    /// Convenience: set clipboard content.
    async fn clipboard_set(
        &self,
        text: &str,
    ) -> Result<ActionResult> {
        self.execute(DesktopAction::ClipboardSet {
            text: text.to_string(),
        })
        .await
    }

    /// Convenience: watch a directory for changes.
    async fn watch_directory(&self, path: &str) -> Result<ActionResult> {
        self.execute(DesktopAction::WatchDirectory {
            path: path.to_string(),
        })
        .await
    }

    /// Convenience: stop watching a directory.
    async fn unwatch_directory(&self, path: &str) -> Result<ActionResult> {
        self.execute(DesktopAction::UnwatchDirectory {
            path: path.to_string(),
        })
        .await
    }

    /// Convenience: watch a single file for changes.
    async fn watch_file(&self, path: &str) -> Result<ActionResult> {
        self.execute(DesktopAction::WatchFile {
            path: path.to_string(),
        })
        .await
    }

    /// Convenience: stop watching a single file.
    async fn unwatch_file(&self, path: &str) -> Result<ActionResult> {
        self.execute(DesktopAction::UnwatchFile {
            path: path.to_string(),
        })
        .await
    }

    /// Convenience: list network sockets.
    async fn list_ports(
        &self,
        filter_protocol: Option<&str>,
        filter_state: Option<&str>,
    ) -> Result<ActionResult> {
        self.execute(DesktopAction::ListPorts {
            filter_protocol: filter_protocol.map(String::from),
            filter_state: filter_state.map(String::from),
        })
        .await
    }

    /// Convenience: test ICMP ping to a host.
    async fn test_ping(&self, target: &str, count: Option<u32>) -> Result<ActionResult> {
        self.execute(DesktopAction::TestPing {
            target: target.to_string(),
            count,
        })
        .await
    }

    /// Convenience: test TCP connectivity to a host:port.
    async fn test_tcp_connect(
        &self,
        target: &str,
        port: u16,
        timeout_ms: Option<u64>,
    ) -> Result<ActionResult> {
        self.execute(DesktopAction::TestTcpConnect {
            target: target.to_string(),
            port,
            timeout_ms,
        })
        .await
    }

    /// Convenience: list firewall rules.
    async fn list_firewall_rules(&self) -> Result<ActionResult> {
        self.execute(DesktopAction::ListFirewallRules).await
    }

    /// Convenience: restart a process by PID or name.
    async fn restart_process(
        &self,
        pid: Option<u32>,
        name: Option<&str>,
        force: bool,
    ) -> Result<ActionResult> {
        self.execute(DesktopAction::RestartProcess {
            pid,
            name: name.map(String::from),
            force,
        })
        .await
    }

    /// Convenience: set process priority.
    async fn set_process_priority(
        &self,
        pid: Option<u32>,
        name: Option<&str>,
        priority: i32,
    ) -> Result<ActionResult> {
        self.execute(DesktopAction::SetProcessPriority {
            pid,
            name: name.map(String::from),
            priority,
        })
        .await
    }
}

/// Create the appropriate adapter for the current platform.
///
/// - macOS → `MacosPhysicalAdapter`
/// - Windows → `WindowsPhysicalAdapter`
/// - Linux + X11 → `X11PhysicalAdapter`
/// - Linux + Wayland → `WaylandPhysicalAdapter`
/// - Linux headless → `HeadlessPhysicalAdapter`
#[allow(unreachable_code)]
pub async fn create_adapter(
    registry: Arc<ToolRegistry>,
) -> Result<Box<dyn ComputerAdapter>> {
    #[cfg(target_os = "macos")]
    {
        return platform_macos::create(registry).await;
    }

    #[cfg(target_os = "windows")]
    {
        return platform_windows::create(registry).await;
    }

    #[cfg(target_os = "linux")]
    {
        return platform_linux::create(registry).await;
    }

    Err(ComputerError::UnsupportedPlatform(
        std::env::consts::OS.to_string(),
    ))
}

/// Detect whether a display server is available at runtime.
pub fn has_display_server() -> bool {
    has_x11() || has_wayland() || cfg!(target_os = "macos") || cfg!(target_os = "windows")
}

/// Detect X11.
pub fn has_x11() -> bool {
    std::env::var("DISPLAY").is_ok() && std::env::var("WAYLAND_DISPLAY").is_err()
}

/// Detect Wayland.
pub fn has_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ui_element_find_by_role() {
        let tree = UiElement {
            id: "root".to_string(),
            role: "window".to_string(),
            label: Some("Main".to_string()),
            value: None,
            bounds: Rect::new(0, 0, 100, 100),
            enabled: true,
            focused: false,
            children: vec![UiElement {
                id: "btn".to_string(),
                role: "button".to_string(),
                label: Some("OK".to_string()),
                value: None,
                bounds: Rect::new(10, 10, 50, 20),
                enabled: true,
                focused: false,
                children: vec![],
            }],
        };

        let btn = tree.find_by_role("button");
        assert!(btn.is_some());
        assert_eq!(btn.unwrap().label, Some("OK".to_string()));
    }

    #[test]
    fn test_ui_element_center() {
        let el = UiElement {
            id: "test".to_string(),
            role: "button".to_string(),
            label: None,
            value: None,
            bounds: Rect::new(10, 20, 100, 50),
            enabled: true,
            focused: false,
            children: vec![],
        };
        assert_eq!(el.center(), Point::new(60, 45));
    }

    #[test]
    fn test_action_result_helpers() {
        let r = ActionResult::success("done").with_data(serde_json::json!({"x": 1}));
        assert!(r.success);
        assert_eq!(r.message, "done");
        assert!(r.data.is_some());
    }

    #[test]
    fn test_click_target_serde() {
        let target = ClickTarget::Coordinate(Point::new(100, 200));
        let json = serde_json::to_string(&target).unwrap();
        assert!(json.contains("coordinate"));
    }
}
