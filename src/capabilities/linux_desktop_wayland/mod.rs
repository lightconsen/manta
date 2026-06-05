//! Linux Wayland desktop capability set — GUI automation via Wayland portals.

use super::{CapabilitySet, OsControlScope, PlatformConstraints};
use crate::tools::Tool;

/// Linux Wayland desktop capability set — provides GUI automation through
/// Wayland-specific mechanisms such as `xdg-desktop-portal`, `grim`,
/// and compositor-specific protocols.
pub struct LinuxDesktopWaylandSet;

impl LinuxDesktopWaylandSet {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxDesktopWaylandSet {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilitySet for LinuxDesktopWaylandSet {
    fn id(&self) -> &str {
        "linux-desktop-wayland"
    }

    fn name(&self) -> &str {
        "Linux Wayland Desktop Control"
    }

    fn description(&self) -> &str {
        "Linux Wayland desktop automation: UI inspection, screenshots, \
         and input simulation via xdg-desktop-portal and compositor APIs."
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
        // TODO: implement Wayland-specific tools (portal screenshot, etc.)
        Vec::new()
    }

    fn is_available(&self) -> bool {
        super::has_wayland()
    }
}
