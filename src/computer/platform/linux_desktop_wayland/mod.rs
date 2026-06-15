//! Linux Wayland desktop capability set — GUI automation via Wayland portals.

pub mod accessibility;
pub mod clipboard;
pub mod desktop_control;
pub mod screenshot;

pub use accessibility::WaylandAccessibilityTool;
pub use clipboard::ClipboardTool;
pub use desktop_control::DesktopControlTool;
pub use screenshot::ScreenshotTool;

use super::{OsControlScope, PlatformConstraints, PlatformToolSet};
use crate::tools::Tool;

/// Linux Wayland desktop platform tool set — provides GUI automation through
/// Wayland-specific mechanisms such as `grim`, `ydotool`, and `wtype`.
pub struct LinuxDesktopWaylandToolset;

impl LinuxDesktopWaylandToolset {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxDesktopWaylandToolset {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformToolSet for LinuxDesktopWaylandToolset {
    fn id(&self) -> &str {
        "wayland"
    }

    fn name(&self) -> &str {
        "Linux Wayland Desktop Control"
    }

    fn description(&self) -> &str {
        "Linux Wayland desktop automation: screenshots (grim/spectacle/gnome-screenshot) \
         and input simulation (ydotool/wtype for click, type, key), \
         and clipboard (wl-copy/wl-paste). \
         Note: Wayland restricts window introspection compared to X11."
    }

    fn constraints(&self) -> &PlatformConstraints {
        static CONSTRAINTS: std::sync::OnceLock<PlatformConstraints> =
            std::sync::OnceLock::new();
        CONSTRAINTS.get_or_init(|| PlatformConstraints {
            target_os: vec!["linux".to_string()],
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
            Box::new(WaylandAccessibilityTool::new()),
        ]
    }

    fn is_available(&self) -> bool {
        super::has_wayland()
    }
}
