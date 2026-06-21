//! Device status event bus.
//!
//! [`DeviceStatusEvent`] is emitted whenever a device's operational status
//! changes (Connected → Degraded → Error → Disconnected).  Consumers
//! subscribe via [`super::registry::DeviceRegistry::subscribe_status`].
//!
//! # Example
//!
//! ```ignore
//! let mut rx = registry.subscribe_status();
//! while let Ok(event) = rx.recv().await {
//!     println!("{} → {:?}", event.device_id, event.current);
//! }
//! ```

use serde::Serialize;

use crate::device::DeviceStatus;

/// A status change event for a single device.
///
/// Emitted every time a device transitions between [`DeviceStatus`]
/// variants — e.g. `Connected → Degraded` on a health check timeout,
/// or `Connected → Disconnected` on hot-plug removal.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceStatusEvent {
    /// The device whose status changed.
    pub device_id: String,
    /// The status before the change.
    pub previous: DeviceStatus,
    /// The status after the change.
    pub current: DeviceStatus,
    /// UNIX epoch milliseconds when the change occurred.
    pub timestamp_millis: u64,
}
