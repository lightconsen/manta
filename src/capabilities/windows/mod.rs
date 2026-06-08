//! Windows capability set — GUI automation and system control tools.

pub mod accessibility;
pub mod clipboard;
pub mod desktop_control;
pub mod powershell;
pub mod screenshot;

pub use accessibility::WindowsAccessibilityTool;
pub use clipboard::ClipboardTool;
pub use desktop_control::DesktopControlTool;
pub use powershell::PowerShellTool;
pub use screenshot::ScreenshotTool;

use super::{CapabilitySet, OsControlScope, PlatformConstraints};
use crate::tools::Tool;

/// Windows capability set — provides desktop automation, screenshots,
/// clipboard access, and PowerShell execution for Windows environments.
pub struct WindowsSet;

impl WindowsSet {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WindowsSet {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilitySet for WindowsSet {
    fn id(&self) -> &str {
        "windows"
    }

    fn name(&self) -> &str {
        "Windows Control"
    }

    fn description(&self) -> &str {
        "Windows desktop automation: screenshots (PowerShell/.NET), \
         desktop control (click, type, key, window management), \
         clipboard access, and PowerShell script execution."
    }

    fn constraints(&self) -> &PlatformConstraints {
        static CONSTRAINTS: std::sync::OnceLock<PlatformConstraints> =
            std::sync::OnceLock::new();
        CONSTRAINTS.get_or_init(|| PlatformConstraints {
            target_os: vec!["windows".to_string()],
            requires_gui: true,
            requires_services: Vec::new(),
        })
    }

    fn scope(&self) -> OsControlScope {
        OsControlScope::UserSpace
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(ScreenshotTool::new()),
            Box::new(DesktopControlTool::new()),
            Box::new(ClipboardTool::new()),
            Box::new(PowerShellTool::new()),
            Box::new(WindowsAccessibilityTool::new()),
        ]
    }

    fn is_available(&self) -> bool {
        cfg!(target_os = "windows")
    }
}
