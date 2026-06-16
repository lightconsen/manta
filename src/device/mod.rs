//! Physical device abstraction layer.
//!
//! This module provides the types and traits for representing and managing
//! physical hardware devices (motors, cameras, sensors, actuators, etc.)
//! that an Agent can discover, control, and monitor.
//!
//! # Architecture
//!
//! ```text
//! DeviceRegistry  ── manages ──▶  Vec<Device>
//!                                     │
//!                            ┌────────┼────────┐
//!                            ▼        ▼        ▼
//!                        DeviceInfo  status  capabilities
//!                            │                  │
//!                            ▼                  ▼
//!                        model / fw         Capability trait
//! ```
//!
//! Each [`Device`] carries its own [`SafetyZone`] and exposes a set of
//! [`Capability`] implementations (one per operation, e.g. `motor.move_to`,
//! `camera.capture`).

use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

pub mod capability;
pub mod driver;
pub mod health;
pub mod hotplug;
pub mod mock;
pub mod os_bridge;
pub mod registry;
pub mod safety;
pub mod status_bus;

pub use capability::{Capability, CapabilityResult, DeviceEvent, ObservableCapability};
pub use driver::{DeviceDriver, DeviceLifecycle};
pub use health::HealthCheckConfig;
pub use hotplug::HotPlugConfig;
pub use mock::{MockCapability, MockDeviceDriver, MockObservableCapability};
pub use registry::{DeviceLock, DeviceRegistry};
pub use safety::{SafetyRule, SafetyRuleKind, SafetyZone};
pub use status_bus::DeviceStatusEvent;

/// Convenience helper: current UNIX epoch time in nanoseconds.
pub fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Static metadata about a physical device.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    /// Unique device identifier, e.g. `"motor-01"`, `"camera-背部"`.
    pub id: String,
    /// Human-readable model name, e.g. `"NEMA-17 Stepper"`.
    pub model: String,
    /// Firmware version string, if available.
    pub firmware_version: Option<String>,
    /// Physical location description, e.g. `"lab-bench-left"`.
    pub location: Option<String>,
}

/// Operational status of a device.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DeviceStatus {
    /// Device is not connected / not present.
    Disconnected,
    /// Device is connected and operating normally.
    Connected {
        /// Timestamp (UNIX epoch nanos) of the last successful connection.
        since: u64,
    },
    /// Device is connected but in an error state.
    Error {
        /// Human-readable error description.
        message: String,
        /// Timestamp of when the error occurred.
        since: u64,
    },
    /// Device is connected but operating in degraded mode.
    Degraded {
        /// Human-readable degradation description.
        message: String,
        /// Timestamp of degradation.
        since: u64,
    },
}

impl Default for DeviceStatus {
    fn default() -> Self {
        Self::Disconnected
    }
}

impl DeviceStatus {
    /// Returns `true` if the device is connected (healthy or degraded).
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. } | Self::Degraded { .. })
    }

    /// Returns `true` if the device is in an error state.
    pub fn has_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    /// Create a `Connected` status with the current timestamp.
    pub fn connected_now() -> Self {
        let since = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self::Connected { since }
    }
}

/// A physical device known to the system.
///
/// Each device has static [`DeviceInfo`], a mutable [`DeviceStatus`],
/// a set of [`Capability`] implementations that the Agent can invoke,
/// and a [`SafetyZone`] for safety-constraint enforcement.
pub struct Device {
    /// Static metadata.
    pub info: DeviceInfo,
    /// Current operational status.
    pub status: Arc<RwLock<DeviceStatus>>,
    /// Capabilities this device exposes (each is a device operation).
    pub capabilities: Vec<Arc<dyn Capability>>,
    /// Safety zone for this device.
    pub safety_zone: Arc<RwLock<SafetyZone>>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("info", &self.info)
            .field("status", &self.status)
            .field("capability_count", &self.capabilities.len())
            .finish()
    }
}

impl Device {
    /// Create a new device.
    pub fn new(
        info: DeviceInfo,
        capabilities: Vec<Arc<dyn Capability>>,
        safety_zone: SafetyZone,
    ) -> Self {
        Self {
            info,
            status: Arc::new(RwLock::new(DeviceStatus::Disconnected)),
            capabilities,
            safety_zone: Arc::new(RwLock::new(safety_zone)),
        }
    }

    /// Create a new device in the `Connected` state.
    pub fn connected(
        info: DeviceInfo,
        capabilities: Vec<Arc<dyn Capability>>,
        safety_zone: SafetyZone,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            info,
            status: Arc::new(RwLock::new(DeviceStatus::Connected { since: now })),
            capabilities,
            safety_zone: Arc::new(RwLock::new(safety_zone)),
        }
    }

    /// Device ID (shortcut for `self.info.id`).
    pub fn id(&self) -> &str {
        &self.info.id
    }

    /// Model name (shortcut for `self.info.model`).
    pub fn model(&self) -> &str {
        &self.info.model
    }

    /// Set the device status to `Connected`.
    pub async fn mark_connected(&self) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut status = self.status.write().await;
        *status = DeviceStatus::Connected { since: now };
    }

    /// Set the device status to `Error`.
    pub async fn mark_error(&self, message: impl Into<String>) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut status = self.status.write().await;
        *status = DeviceStatus::Error {
            message: message.into(),
            since: now,
        };
    }

    /// Set the device status to `Degraded`.
    pub async fn mark_degraded(&self, message: impl Into<String>) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut status = self.status.write().await;
        *status = DeviceStatus::Degraded {
            message: message.into(),
            since: now,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_creation() {
        let info = DeviceInfo {
            id: "motor-01".into(),
            model: "NEMA-17".into(),
            firmware_version: None,
            location: None,
        };
        let device = Device::new(info.clone(), vec![], SafetyZone::new(vec![]));
        assert_eq!(device.id(), "motor-01");
        assert_eq!(device.model(), "NEMA-17");
        assert_eq!(device.info.id, "motor-01");
    }

    #[test]
    fn test_device_connected_state() {
        let info = DeviceInfo {
            id: "cam-01".into(),
            model: "Camera".into(),
            firmware_version: None,
            location: None,
        };
        let device = Device::connected(info, vec![], SafetyZone::new(vec![]));
        assert!(device.status.blocking_read().is_connected());
    }

    #[tokio::test]
    async fn test_device_status_transitions() {
        let info = DeviceInfo {
            id: "sensor-01".into(),
            model: "TempSensor".into(),
            firmware_version: None,
            location: None,
        };
        let device = Device::new(info, vec![], SafetyZone::new(vec![]));

        // Initially disconnected
        assert!(!device.status.read().await.is_connected());

        // Mark connected
        device.mark_connected().await;
        assert!(device.status.read().await.is_connected());

        // Mark error
        device.mark_error("overheated").await;
        assert!(device.status.read().await.has_error());
        assert!(!device.status.read().await.is_connected());

        // Mark degraded
        device.mark_degraded("high latency").await;
        let status = device.status.read().await;
        assert!(status.is_connected());
        assert!(!status.has_error());
        match &*status {
            DeviceStatus::Degraded { message, .. } => {
                assert_eq!(message, "high latency");
            }
            _ => panic!("expected Degraded"),
        }
    }

    #[test]
    fn test_device_status_default() {
        let status = DeviceStatus::default();
        assert!(matches!(status, DeviceStatus::Disconnected));
    }

    #[test]
    fn test_device_info_serialization() {
        let info = DeviceInfo {
            id: "dev-01".into(),
            model: "TestDevice".into(),
            firmware_version: Some("1.0.0".into()),
            location: Some("rack-3".into()),
        };
        let json = serde_json::to_string(&info).unwrap();
        let deserialized: DeviceInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "dev-01");
        assert_eq!(deserialized.firmware_version, Some("1.0.0".into()));
    }
}
