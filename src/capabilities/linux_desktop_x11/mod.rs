//! Linux X11 desktop capability set — GUI automation via X11 protocols.

use super::{CapabilitySet, OsControlScope, PlatformConstraints};
use crate::tools::Tool;

/// Linux X11 desktop capability set — provides GUI automation through
/// X11-specific tools such as `xdotool`, `xclip`, and `xwd`/`maim`.
pub struct LinuxDesktopX11Set;

impl LinuxDesktopX11Set {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxDesktopX11Set {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilitySet for LinuxDesktopX11Set {
    fn id(&self) -> &str {
        "linux-desktop-x11"
    }

    fn name(&self) -> &str {
        "Linux X11 Desktop Control"
    }

    fn description(&self) -> &str {
        "Linux X11 desktop automation: UI inspection, screenshots, \
         keyboard/mouse simulation via X11 protocols."
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
        // TODO: implement X11-specific tools (xdotool, x11 screenshot, etc.)
        Vec::new()
    }

    fn is_available(&self) -> bool {
        super::has_x11()
    }
}
