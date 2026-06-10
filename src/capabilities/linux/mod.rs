//! Linux capability set — system management tools for Linux.

pub mod cron_manager;
pub mod firewall_manager;
pub mod log_analyzer;
pub mod network_diag;
pub mod package_manager;
pub mod server_operator;
pub mod service_manager;
pub mod system_inspect;
pub mod user_manager;

pub use cron_manager::CronManagerTool;
pub use firewall_manager::FirewallManagerTool;
pub use log_analyzer::LogAnalyzerTool;
pub use network_diag::NetworkDiagTool;
pub use package_manager::PackageManagerTool;
pub use service_manager::ServiceManagerTool;
pub use system_inspect::SystemInspectTool;
pub use user_manager::UserManagerTool;

use super::{CapabilitySet, OsControlScope, PlatformConstraints};
use crate::tools::Tool;

/// Linux capability set — provides system inspection and service
/// management tools for Linux environments (server and desktop).
pub struct LinuxSet;

impl LinuxSet {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinuxSet {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilitySet for LinuxSet {
    fn id(&self) -> &str {
        "linux"
    }

    fn name(&self) -> &str {
        "Linux Control"
    }

    fn description(&self) -> &str {
        "Linux system management: system inspection, \
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
            Box::new(LogAnalyzerTool::new()),
            Box::new(NetworkDiagTool::new()),
            Box::new(PackageManagerTool::new()),
            Box::new(FirewallManagerTool::new()),
            Box::new(UserManagerTool::new()),
            Box::new(CronManagerTool::new()),
        ]
    }
}
