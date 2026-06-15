//! Linux X11 desktop capability set — GUI automation via X11 protocols.

pub mod accessibility;
pub mod clipboard;
pub mod desktop_control;
pub mod screenshot;

pub use accessibility::X11AccessibilityTool;
pub use clipboard::ClipboardTool;
pub use desktop_control::DesktopControlTool;
pub use screenshot::ScreenshotTool;

use super::{OsControlScope, PlatformConstraints, PlatformToolSet};
use crate::tools::Tool;

/// Linux X11 desktop platform tool set — provides GUI automation through
/// X11-specific tools such as `xdotool`, `xclip`, `maim`, and `import`.
pub struct LinuxDesktopX11Toolset;

impl LinuxDesktopX11Toolset {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxDesktopX11Toolset {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformToolSet for LinuxDesktopX11Toolset {
    fn id(&self) -> &str {
        "x11"
    }

    fn name(&self) -> &str {
        "Linux X11 Desktop Control"
    }

    fn description(&self) -> &str {
        "Linux X11 desktop automation: screenshots (maim/import/gnome-screenshot), \
         desktop control (xdotool click/type/key/window management), \
         and clipboard (xclip)."
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
            Box::new(X11AccessibilityTool::new()),
        ]
    }

    fn is_available(&self) -> bool {
        super::has_x11()
    }
}
