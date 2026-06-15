//! Linux implementation of SystemInspector for ServerOperator.

use crate::computer::capabilities::server_operator::{SystemInspector, SystemSnapshot};
use crate::computer::capabilities::linux::system_inspect::SystemInspectTool;

/// Linux-specific system inspector.
#[derive(Debug, Default)]
pub struct LinuxSystemInspector;

impl LinuxSystemInspector {
    /// Create a new inspector.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl SystemInspector for LinuxSystemInspector {
    async fn inspect_full(&self) -> crate::Result<SystemSnapshot> {
        let (hostname, uptime, load_avg, memory, cpu_count) =
            SystemInspectTool::collect_overview().await;

        let (disks, processes, services, listening_ports, recent_logs) = tokio::join!(
            SystemInspectTool::collect_storage(),
            SystemInspectTool::collect_processes(20),
            SystemInspectTool::collect_services(20),
            SystemInspectTool::collect_network(),
            SystemInspectTool::collect_logs(30, "1 hour ago"),
        );

        Ok(SystemSnapshot {
            hostname,
            uptime,
            load_average: load_avg,
            memory,
            cpu_count,
            disks,
            processes,
            services,
            listening_ports,
            recent_logs,
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
    }
}
