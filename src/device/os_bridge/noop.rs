//! No-op OS device monitor — used on platforms without native event support.
//!
//! Returns a closed broadcast channel so that the bridge loop exits
//! immediately without consuming resources.

use tokio::sync::broadcast;

use super::{OsDeviceEvent, OsDeviceMonitor};

/// A monitor that produces no events.
pub struct NoopOsMonitor {
    tx: broadcast::Sender<OsDeviceEvent>,
}

impl NoopOsMonitor {
    /// Create a new no-op monitor.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1);
        Self { tx }
    }
}

impl Default for NoopOsMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl OsDeviceMonitor for NoopOsMonitor {
    fn subscribe(&self) -> broadcast::Receiver<OsDeviceEvent> {
        self.tx.subscribe()
    }
}

/// Factory function (matches the signature used by platform module).
pub fn create_os_monitor() -> impl OsDeviceMonitor {
    NoopOsMonitor::new()
}
