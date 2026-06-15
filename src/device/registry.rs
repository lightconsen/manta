//! Registry for discovering, registering, and managing devices.
//!
//! [`DeviceRegistry`] holds all registered [`DeviceDriver`](super::DeviceDriver)
//! instances and the [`Device`](super::Device) objects they produce.

use crate::device::driver::DeviceDriver;
use crate::device::{Device, DeviceStatus};
use crate::error::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Registry of device drivers and their connected devices.
///
/// # Lifecycle
///
/// 1. Register drivers via [`register`](Self::register).
/// 2. Call [`probe_all`](Self::probe_all) to discover available hardware.
/// 3. Call [`connect`](Self::connect) to initialize a specific device.
/// 4. Call [`health_check`](Self::health_check) periodically to monitor.
/// 5. Call [`disconnect`](Self::disconnect) on shutdown.
#[derive(Default)]
pub struct DeviceRegistry {
    drivers: Vec<Arc<dyn DeviceDriver>>,
    devices: RwLock<HashMap<String, DeviceEntry>>,
}

impl std::fmt::Debug for DeviceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceRegistry")
            .field("drivers", &self.drivers.len())
            .finish()
    }
}

#[derive(Clone)]
struct DeviceEntry {
    device: Arc<Device>,
    /// Index into `self.drivers` for the driver that created this device.
    driver_idx: usize,
}

impl DeviceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            drivers: Vec::new(),
            devices: RwLock::new(HashMap::new()),
        }
    }

    /// Register a device driver.
    ///
    /// Drivers are probed in registration order during [`probe_all`](Self::probe_all).
    pub fn register(&mut self, driver: Arc<dyn DeviceDriver>) {
        self.drivers.push(driver);
    }

    /// Probe all registered drivers and return those whose hardware is present.
    ///
    /// This does **not** connect to the devices — only reports availability.
    pub async fn probe_all(&self) -> Result<Vec<String>> {
        let mut available = Vec::new();
        for driver in &self.drivers {
            match driver.probe().await {
                Ok(true) => available.push(driver.driver_name().to_string()),
                Ok(false) => { /* not present, skip */ }
                Err(e) => {
                    tracing::warn!(
                        "Device probe error for '{}': {}",
                        driver.driver_name(),
                        e
                    );
                }
            }
        }
        Ok(available)
    }

    /// Connect to a device by driver name.
    ///
    /// The resulting [`Device`] is stored in the registry and can be
    /// retrieved via [`get`](Self::get).
    ///
    /// Returns an error if the driver is not registered or connection fails.
    pub async fn connect(&self, driver_name: &str) -> Result<Arc<Device>> {
        let (driver_idx, driver) = self
            .drivers
            .iter()
            .enumerate()
            .find(|(_, d)| d.driver_name() == driver_name)
            .ok_or_else(|| {
                crate::error::SyscityError::NotFound {
                    resource: format!("Device driver '{}'", driver_name),
                }
            })?;

        let device = driver.connect().await?;
        let id = device.id().to_string();
        let device = Arc::new(device);

        let mut devices = self.devices.write().await;
        devices.insert(
            id.clone(),
            DeviceEntry {
                device: device.clone(),
                driver_idx,
            },
        );
        tracing::info!("Device connected: {} ({})", id, driver_name);
        Ok(device)
    }

    /// Get a connected device by its ID.
    pub async fn get(&self, device_id: &str) -> Option<Arc<Device>> {
        self.devices
            .read()
            .await
            .get(device_id)
            .map(|e| e.device.clone())
    }

    /// List all connected device IDs.
    pub async fn list(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.devices.read().await.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Number of connected devices.
    pub async fn len(&self) -> usize {
        self.devices.read().await.len()
    }

    /// Returns `true` if no devices are connected.
    pub async fn is_empty(&self) -> bool {
        self.devices.read().await.is_empty()
    }

    /// Run a health check on a specific device.
    ///
    /// Updates the device's status to `Degraded` if the health check fails.
    pub async fn health_check(&self, device_id: &str) -> Result<bool> {
        let entry = self
            .devices
            .read()
            .await
            .get(device_id)
            .cloned()
            .ok_or_else(|| crate::error::SyscityError::NotFound {
                resource: format!("Device '{}'", device_id),
            })?;

        let driver = &self.drivers[entry.driver_idx];
        let healthy = driver.health_check().await.unwrap_or(false);

        if !healthy {
            entry
                .device
                .mark_degraded(format!("Health check failed for '{}'", device_id))
                .await;
        }

        Ok(healthy)
    }

    /// Run health checks on all connected devices.
    ///
    /// Returns a map of device IDs to health status.
    pub async fn health_check_all(&self) -> HashMap<String, bool> {
        let ids: Vec<String> = self.list().await;
        let mut results = HashMap::new();
        for id in ids {
            let healthy = self.health_check(&id).await.unwrap_or(false);
            results.insert(id, healthy);
        }
        results
    }

    /// Disconnect a specific device.
    pub async fn disconnect(&self, device_id: &str) -> Result<()> {
        let mut devices = self.devices.write().await;
        if let Some(entry) = devices.remove(device_id) {
            entry
                .device
                .status
                .write()
                .await
                .clone_from(&DeviceStatus::Disconnected);
            tracing::info!("Device disconnected: {}", device_id);
        }
        Ok(())
    }

    /// Disconnect all devices.
    pub async fn disconnect_all(&self) {
        let mut devices = self.devices.write().await;
        for (id, entry) in devices.drain() {
            entry
                .device
                .status
                .write()
                .await
                .clone_from(&DeviceStatus::Disconnected);
            tracing::info!("Device disconnected: {}", id);
        }
    }

    /// Get the number of registered drivers.
    pub fn driver_count(&self) -> usize {
        self.drivers.len()
    }

    /// List registered driver names.
    pub fn driver_names(&self) -> Vec<String> {
        self.drivers
            .iter()
            .map(|d| d.driver_name().to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::safety::{SafetyRule, SafetyRuleKind, SafetyZone};
    use crate::device::DeviceInfo;
    use async_trait::async_trait;

    struct MockDriver {
        name: String,
        present: bool,
    }

    #[async_trait]
    impl DeviceDriver for MockDriver {
        fn driver_name(&self) -> &str {
            &self.name
        }

        async fn probe(&self) -> Result<bool> {
            Ok(self.present)
        }

        async fn connect(&self) -> Result<Device> {
            // Check probe first — simulate absent hardware
            if !self.probe().await.unwrap_or(false) {
                return Err(crate::error::SyscityError::NotFound {
                    resource: format!("Device '{}' not present", self.name),
                });
            }
            let info = DeviceInfo {
                id: format!("dev-{}", self.name),
                model: self.name.clone(),
                firmware_version: None,
                location: None,
            };
            Ok(Device::connected(
                info,
                vec![],
                SafetyZone::new(vec![SafetyRule {
                    name: "default".into(),
                    kind: SafetyRuleKind::RequiresApproval,
                }]),
            ))
        }
    }

    #[tokio::test]
    async fn test_empty_registry() {
        let reg = DeviceRegistry::new();
        assert!(reg.is_empty().await);
        assert_eq!(reg.len().await, 0);
        assert!(reg.driver_names().is_empty());
    }

    #[tokio::test]
    async fn test_probe_and_connect() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }));
        reg.register(Arc::new(MockDriver {
            name: "camera".into(),
            present: false, // not present
        }));

        let available = reg.probe_all().await.unwrap();
        assert_eq!(available, vec!["motor"]);

        let device = reg.connect("motor").await.unwrap();
        assert_eq!(device.id(), "dev-motor");
        assert_eq!(reg.len().await, 1);

        // Camera is absent — connecting should fail
        let err = reg.connect("camera").await.unwrap_err();
        assert!(err.to_string().contains("not present"));
    }

    #[tokio::test]
    async fn test_connect_unknown_driver() {
        let reg = DeviceRegistry::new();
        let err = reg.connect("nonexistent").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_list_and_get() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "sensor-a".into(),
            present: true,
        }));
        reg.register(Arc::new(MockDriver {
            name: "sensor-b".into(),
            present: true,
        }));

        reg.connect("sensor-a").await.unwrap();
        reg.connect("sensor-b").await.unwrap();

        let ids = reg.list().await;
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"dev-sensor-a".into()));
        assert!(ids.contains(&"dev-sensor-b".into()));

        let device = reg.get("dev-sensor-a").await;
        assert!(device.is_some());
        assert_eq!(device.unwrap().model(), "sensor-a");
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }));
        reg.connect("motor").await.unwrap();
        assert_eq!(reg.len().await, 1);

        reg.disconnect("dev-motor").await.unwrap();
        assert!(reg.is_empty().await);
    }

    #[tokio::test]
    async fn test_disconnect_all() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "a".into(),
            present: true,
        }));
        reg.register(Arc::new(MockDriver {
            name: "b".into(),
            present: true,
        }));
        reg.connect("a").await.unwrap();
        reg.connect("b").await.unwrap();
        assert_eq!(reg.len().await, 2);

        reg.disconnect_all().await;
        assert!(reg.is_empty().await);
    }

    #[tokio::test]
    async fn test_health_check() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }));
        reg.connect("motor").await.unwrap();

        // Mock driver health check always returns Ok(true)
        let healthy = reg.health_check("dev-motor").await.unwrap();
        assert!(healthy);
    }

    #[test]
    fn test_driver_count_and_names() {
        let mut reg = DeviceRegistry::new();
        assert_eq!(reg.driver_count(), 0);
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }));
        reg.register(Arc::new(MockDriver {
            name: "camera".into(),
            present: false,
        }));
        assert_eq!(reg.driver_count(), 2);
        let names = reg.driver_names();
        assert!(names.contains(&"motor".into()));
        assert!(names.contains(&"camera".into()));
    }
}
