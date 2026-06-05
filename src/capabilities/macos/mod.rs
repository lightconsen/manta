//! macOS capability set — GUI automation and desktop control tools.

pub mod accessibility;
pub mod applescript;
pub mod desktop_control;
pub mod screenshot;

pub use accessibility::AccessibilityTool;
pub use applescript::AppleScriptTool;
pub use desktop_control::DesktopControlTool;
pub use screenshot::ScreenshotTool;

use super::{CapabilitySet, OsControlScope, PlatformConstraints};
use crate::tools::Tool;

/// macOS capability set — provides GUI automation, accessibility
/// querying, screenshots, and AppleScript execution for macOS environments.
pub struct MacosSet;

impl MacosSet {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacosSet {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilitySet for MacosSet {
    fn id(&self) -> &str {
        "macos"
    }

    fn name(&self) -> &str {
        "macOS Control"
    }

    fn description(&self) -> &str {
        "macOS desktop automation: accessibility UI tree queries, \
         screenshots, AppleScript execution, and hybrid desktop control \
         (inspect, click, type, keyboard shortcuts)."
    }

    fn constraints(&self) -> &PlatformConstraints {
        static CONSTRAINTS: std::sync::OnceLock<PlatformConstraints> =
            std::sync::OnceLock::new();
        CONSTRAINTS.get_or_init(|| PlatformConstraints {
            target_os: vec!["macos".to_string()],
            requires_gui: true,
            requires_services: Vec::new(),
        })
    }

    fn scope(&self) -> OsControlScope {
        OsControlScope::UserSpace
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(AccessibilityTool::new()),
            Box::new(ScreenshotTool::new()),
            Box::new(AppleScriptTool::new()),
            Box::new(DesktopControlTool::new()),
        ]
    }
}
