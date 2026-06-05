//! Linux Server capability set — headless system management tools.

pub mod service_manager;
pub mod system_inspect;

pub use service_manager::ServiceManagerTool;
pub use system_inspect::SystemInspectTool;

use super::{CapabilitySet, OsControlScope, PlatformConstraints};
use crate::tools::Tool;

/// Linux Server capability set — provides system inspection and service
/// management tools for headless Linux environments.
pub struct LinuxServerSet;

impl LinuxServerSet {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxServerSet {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilitySet for LinuxServerSet {
    fn id(&self) -> &str {
        "linux-server"
    }

    fn name(&self) -> &str {
        "Linux Server Control"
    }

    fn description(&self) -> &str {
        "Headless Linux server management: system inspection, \
         systemd services, logs, network, packages, and users."
    }

    fn constraints(&self) -> &PlatformConstraints {
        static CONSTRAINTS: std::sync::OnceLock<PlatformConstraints> =
            std::sync::OnceLock::new();
        CONSTRAINTS.get_or_init(|| PlatformConstraints {
            target_os: vec!["linux".to_string()],
            requires_gui: false,
            requires_services: vec!["systemd".to_string()],
        })
    }

    fn scope(&self) -> OsControlScope {
        OsControlScope::System
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(SystemInspectTool::new()),
            Box::new(ServiceManagerTool::new()),
        ]
    }
}
