//! Cross-platform system monitoring and process management.
//!
//! Uses the `sysinfo` crate to provide structured CPU, memory, disk,
//! network, and process information without shelling out to `ps`, `df`,
//! `free`, or `tasklist`.

use crate::computer::types::{DiskStatus, NetworkStatus, ProcessEntry, SystemStatus};
use crate::computer::{ComputerError, Result};
use sysinfo::{Disks, Networks, ProcessRefreshKind, RefreshKind, Signal, System};

/// Lightweight system monitor backed by `sysinfo::System`.
///
/// Create a new instance for each query (or keep one around and call
/// `refresh_all()` before reading).
pub struct SystemMonitor {
    system: System,
    networks: Networks,
    disks: Disks,
}

impl SystemMonitor {
    /// Create a monitor with all information freshly loaded.
    pub fn new() -> Self {
        let system = System::new_with_specifics(RefreshKind::everything());
        let networks = Networks::new_with_refreshed_list();
        let disks = Disks::new_with_refreshed_list();
        Self {
            system,
            networks,
            disks,
        }
    }

    /// Refresh and return a full system status snapshot.
    pub fn get_status(&mut self) -> SystemStatus {
        self.system.refresh_all();
        self.networks.refresh();
        self.disks.refresh();

        let cpu_usage = if self.system.cpus().is_empty() {
            0.0
        } else {
            self.system
                .cpus()
                .iter()
                .map(|c| c.cpu_usage())
                .sum::<f32>()
                / self.system.cpus().len() as f32
        };

        let disks: Vec<DiskStatus> = self
            .disks
            .list()
            .iter()
            .map(|d| {
                let total = d.total_space();
                let available = d.available_space();
                let used = total.saturating_sub(available);
                DiskStatus {
                    name: d.name().to_string_lossy().to_string(),
                    mount_point: d.mount_point().to_string_lossy().to_string(),
                    total_gb: bytes_to_gb(total),
                    used_gb: bytes_to_gb(used),
                    available_gb: bytes_to_gb(available),
                }
            })
            .collect();

        let networks: Vec<NetworkStatus> = self
            .networks
            .iter()
            .map(|(name, data)| NetworkStatus {
                name: name.clone(),
                rx_bytes: data.total_received(),
                tx_bytes: data.total_transmitted(),
            })
            .collect();

        SystemStatus {
            hostname: System::host_name().unwrap_or_default(),
            os_name: System::name().unwrap_or_default(),
            os_version: System::os_version().unwrap_or_default(),
            kernel_version: System::kernel_version().unwrap_or_default(),
            uptime_seconds: System::uptime(),
            cpu_usage_percent: cpu_usage,
            cpu_count: self.system.cpus().len(),
            memory_total_mb: bytes_to_mb(self.system.total_memory()),
            memory_used_mb: bytes_to_mb(self.system.used_memory()),
            memory_available_mb: bytes_to_mb(self.system.available_memory()),
            swap_total_mb: bytes_to_mb(self.system.total_swap()),
            swap_used_mb: bytes_to_mb(self.system.used_swap()),
            disks,
            networks,
        }
    }

    /// Refresh and return a list of processes, optionally filtered by name.
    pub fn list_processes(
        &mut self,
        filter: Option<&str>,
        limit: Option<usize>,
    ) -> Vec<ProcessEntry> {
        self.system
            .refresh_processes_specifics(ProcessRefreshKind::everything());

        let mut entries: Vec<ProcessEntry> = self
            .system
            .processes()
            .values()
            .filter_map(|p| {
                let name = p.name().to_string();
                if let Some(f) = filter {
                    if !name.to_lowercase().contains(&f.to_lowercase()) {
                        return None;
                    }
                }
                let status = match p.status() {
                    sysinfo::ProcessStatus::Run => "Running",
                    sysinfo::ProcessStatus::Sleep => "Sleeping",
                    sysinfo::ProcessStatus::Stop => "Stopped",
                    sysinfo::ProcessStatus::Idle => "Idle",
                    sysinfo::ProcessStatus::Zombie => "Zombie",
                    _ => "Unknown",
                }
                .to_string();

                Some(ProcessEntry {
                    pid: p.pid().as_u32(),
                    name,
                    cpu_percent: p.cpu_usage(),
                    memory_mb: bytes_to_mb(p.memory()),
                    status,
                    start_time: {
                        let ts = p.start_time() as i64;
                        chrono::DateTime::from_timestamp(ts, 0)
                            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    },
                })
            })
            .collect();

        // Sort by CPU usage descending
        entries.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some(l) = limit {
            entries.truncate(l);
        }

        entries
    }

    /// Kill a process by PID or name.
    ///
    /// If both `pid` and `name` are provided, PID takes precedence.
    /// If `force` is true, sends SIGKILL (Unix) / force kill (Windows);
    /// otherwise sends SIGTERM (Unix) / normal kill (Windows).
    ///
    /// Returns the PID of the killed process.
    pub fn kill_process(
        &mut self,
        pid: Option<u32>,
        name: Option<&str>,
        force: bool,
    ) -> Result<u32> {
        self.system
            .refresh_processes_specifics(ProcessRefreshKind::new().with_cpu());

        let target_pid = if let Some(p) = pid {
            p
        } else if let Some(n) = name {
            self.system
                .processes()
                .values()
                .find(|p| p.name().to_lowercase().contains(&n.to_lowercase()))
                .map(|p| p.pid().as_u32())
                .ok_or_else(|| {
                    ComputerError::ProcessNotFound(format!(
                        "No process matching '{}' found",
                        n
                    ))
                })?
        } else {
            return Err(ComputerError::ProcessNotFound(
                "Either pid or name must be provided".to_string(),
            ));
        };

        let sys_pid = sysinfo::Pid::from(target_pid as usize);
        let process = self
            .system
            .process(sys_pid)
            .ok_or_else(|| {
                ComputerError::ProcessNotFound(format!("Process {} not found", target_pid))
            })?;

        let signal = if force { Signal::Kill } else { Signal::Term };

        match process.kill_with(signal) {
            Some(true) => Ok(target_pid),
            Some(false) => Err(ComputerError::KillFailed(format!(
                "Failed to kill process {}",
                target_pid
            ))),
            None => Err(ComputerError::KillFailed(format!(
                "Signal {:?} not supported on this platform",
                signal
            ))),
        }
    }
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

fn bytes_to_mb(bytes: u64) -> u64 {
    bytes / 1_024 / 1_024
}

fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / 1_024.0 / 1_024.0 / 1_024.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_monitor_creation() {
        let monitor = SystemMonitor::new();
        // Just verify it doesn't panic
        let _ = monitor;
    }

    #[test]
    fn test_get_status() {
        let mut monitor = SystemMonitor::new();
        let status = monitor.get_status();
        assert!(!status.hostname.is_empty());
        assert!(status.cpu_count > 0);
        assert!(status.memory_total_mb > 0);
    }

    #[test]
    fn test_list_processes() {
        let mut monitor = SystemMonitor::new();
        let procs = monitor.list_processes(None, None);
        assert!(!procs.is_empty());

        // Current process must be in the list
        let current_pid = std::process::id();
        assert!(procs.iter().any(|p| p.pid == current_pid));
    }

    #[test]
    fn test_list_processes_filter() {
        let mut monitor = SystemMonitor::new();
        let all = monitor.list_processes(None, Some(10));
        assert!(!all.is_empty());

        // Filter by a name that definitely doesn't exist
        let filtered = monitor.list_processes(Some("xyzzy_nonexistent_12345"), None);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_list_processes_limit() {
        let mut monitor = SystemMonitor::new();
        let procs = monitor.list_processes(None, Some(5));
        assert!(procs.len() <= 5);
    }

    #[test]
    fn test_kill_process_by_name_not_found() {
        let mut monitor = SystemMonitor::new();
        let result = monitor.kill_process(None, Some("xyzzy_nonexistent_12345"), false);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_kill_process_spawn_and_kill() {
        // Spawn a sleep process
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("failed to spawn sleep");

        let pid = child.id().unwrap();

        // Small delay to let the process appear
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let mut monitor = SystemMonitor::new();
        let result = monitor.kill_process(Some(pid), None, false);
        assert!(result.is_ok(), "kill failed: {:?}", result);
        assert_eq!(result.unwrap(), pid);

        // Wait for the child to actually exit
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            child.wait(),
        )
        .await;
    }
}
