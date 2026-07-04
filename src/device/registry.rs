//! Registry for discovering, registering, and managing devices.
//!
//! [`DeviceRegistry`] holds all registered
//! [`DeviceDriver`](super::DeviceDriver) instances and the
//! [`Device`](super::Device) objects they produce.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::{broadcast, Mutex, RwLock};

use crate::device::driver::DeviceDriver;
use crate::device::status_bus::DeviceStatusEvent;
use crate::device::{Device, DeviceStatus};
use crate::error::{Result, SyscityError};

/// RAII guard that provides exclusive access to a device.
///
/// Obtained via [`DeviceRegistry::try_lock`]. The lock is released
/// automatically when the guard is dropped.
pub struct DeviceLock {
    device: Arc<Device>,
    _guard: tokio::sync::OwnedRwLockWriteGuard<()>,
}

impl DeviceLock {
    /// Access the underlying device.
    pub fn device(&self) -> &Device {
        &self.device
    }
}

impl std::fmt::Debug for DeviceLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceLock")
            .field("device_id", &self.device.id())
            .finish()
    }
}

/// Registry of device drivers and their connected devices.
///
/// # Lifecycle
///
/// 1. Register drivers via [`register`](Self::register).
/// 2. Call [`probe_all`](Self::probe_all) to discover available hardware.
/// 3. Call [`connect`](Self::connect) to initialize a specific device.
/// 4. Call [`health_check`](Self::health_check) periodically to monitor.
/// 5. Call [`disconnect`](Self::disconnect) on shutdown.
///
/// # Status bus
///
/// Call [`subscribe_status`](Self::subscribe_status) to receive
/// [`DeviceStatusEvent`]s whenever a device transitions between
/// connected / degraded / error / disconnected states.
pub struct DeviceRegistry {
    drivers: Mutex<Vec<Arc<dyn DeviceDriver>>>,
    devices: RwLock<HashMap<String, DeviceEntry>>,
    /// Per-device locks for exclusive access (e.g. during calibration,
    /// firmware update, or sequential command sequences).
    locks: RwLock<HashMap<String, Arc<tokio::sync::RwLock<()>>>>,
    /// Broadcast channel for device status change events.
    status_tx: broadcast::Sender<DeviceStatusEvent>,
}

impl std::fmt::Debug for DeviceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let driver_count = self.drivers.try_lock().map(|v| v.len()).unwrap_or(0);
        f.debug_struct("DeviceRegistry")
            .field("drivers", &driver_count)
            .finish()
    }
}

#[derive(Clone)]
struct DeviceEntry {
    device: Arc<Device>,
    /// Index into `self.drivers` for the driver that created this device.
    driver_idx: usize,
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceRegistry {
    /// Create an empty registry.
    ///
    /// The status broadcast channel has a capacity of 256 events.
    pub fn new() -> Self {
        let (status_tx, _) = broadcast::channel(256);
        Self {
            drivers: Mutex::new(Vec::new()),
            devices: RwLock::new(HashMap::new()),
            locks: RwLock::new(HashMap::new()),
            status_tx,
        }
    }

    /// Register a device driver.
    ///
    /// Drivers are probed in registration order during
    /// [`probe_all`](Self::probe_all).
    pub async fn register(&self, driver: Arc<dyn DeviceDriver>) {
        self.drivers.lock().await.push(driver);
    }

    /// Probe all registered drivers and return those whose hardware is present.
    ///
    /// This does **not** connect to the devices — only reports availability.
    pub async fn probe_all(&self) -> Result<Vec<String>> {
        let mut available = Vec::new();
        let snapshot = self.drivers.lock().await.clone();
        for driver in &snapshot {
            match driver.probe().await {
                Ok(true) => available.push(driver.driver_name().to_string()),
                Ok(false) => { /* not present, skip */ }
                Err(e) => {
                    tracing::warn!("Device probe error for '{}': {}", driver.driver_name(), e);
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
        let (driver_idx, driver) = {
            let drivers = self.drivers.lock().await;
            let (idx, d) = drivers
                .iter()
                .enumerate()
                .find(|(_, d)| d.driver_name() == driver_name)
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!("Device driver '{}'", driver_name),
                })?;
            (idx, d.clone())
        };

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

        // Create a lock entry for this device
        self.locks
            .write()
            .await
            .insert(id.clone(), Arc::new(RwLock::new(())));

        tracing::info!("Device connected: {} ({})", id, driver_name);
        // Drop the devices guard so emit_status_event can read them.
        drop(devices);
        self.emit_status_event(&id, DeviceStatus::Disconnected)
            .await;
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

        let driver = self.drivers.lock().await[entry.driver_idx].clone();
        let healthy = driver.health_check().await.unwrap_or(false);

        if !healthy {
            let previous = entry.device.status.read().await.clone();
            entry
                .device
                .mark_degraded(format!("Health check failed for '{}'", device_id))
                .await;
            drop(entry);
            self.emit_status_event(device_id, previous).await;
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
    ///
    /// Emits a [`DeviceStatusEvent`] with the previous → `Disconnected`
    /// transition on the status bus.
    pub async fn disconnect(&self, device_id: &str) -> Result<()> {
        let previous = {
            let mut devices = self.devices.write().await;
            let entry = devices.remove(device_id);
            let prev = match entry {
                Some(ref e) => e.device.status.read().await.clone(),
                None => DeviceStatus::Disconnected,
            };
            if let Some(e) = entry {
                e.device
                    .status
                    .write()
                    .await
                    .clone_from(&DeviceStatus::Disconnected);
                tracing::info!("Device disconnected: {}", device_id);
            }
            prev
        };
        self.locks.write().await.remove(device_id);
        self.emit_status_event(device_id, previous).await;
        Ok(())
    }

    /// Disconnect all devices.
    ///
    /// Emits a [`DeviceStatusEvent`] for each disconnected device.
    pub async fn disconnect_all(&self) {
        let ids: Vec<String> = self.list().await;
        for id in &ids {
            let _ = self.disconnect(id).await;
        }
    }

    /// Try to acquire exclusive access to a connected device.
    ///
    /// Returns `None` if the device is not connected or if the lock
    /// is already held. The lock is released when the returned
    /// [`DeviceLock`] is dropped.
    pub async fn try_lock(&self, device_id: &str) -> Option<DeviceLock> {
        let device = self.get(device_id).await?;
        let lock = self.locks.read().await.get(device_id)?.clone();
        let guard = lock.try_write_owned().ok()?;
        Some(DeviceLock { device, _guard: guard })
    }

    /// Subscribe to device status change events.
    ///
    /// Returns a `broadcast::Receiver` that yields [`DeviceStatusEvent`]s
    /// every time a device transitions between operational states.
    /// Capacity is 256 events; slow consumers that fall behind will miss
    /// events (the oldest are dropped).
    pub fn subscribe_status(&self) -> broadcast::Receiver<DeviceStatusEvent> {
        self.status_tx.subscribe()
    }

    /// Update a device's status and emit a [`DeviceStatusEvent`].
    ///
    /// This is the preferred way to change a device's status when the
    /// caller wants the change to be observable.  For low-level direct
    /// mutations use [`Device::mark_connected`] etc. directly.
    pub async fn set_device_status(&self, device_id: &str, new_status: DeviceStatus) {
        if let Some(device) = self.get(device_id).await {
            let mut status = device.status.write().await;
            let previous = status.clone();
            *status = new_status;
            drop(status);
            self.emit_status_event(device_id, previous).await;
        }
    }

    /// Reconnect a device by disconnecting and re-connecting through its
    /// driver.
    ///
    /// The old device is disconnected, a fresh [`Device`] object is created
    /// via the driver's [`connect`](DeviceDriver::connect), and the new
    /// device takes its place in the registry.  A `Disconnected` + `Connected`
    /// pair of events is emitted.
    pub async fn reconnect(&self, device_id: &str) -> Result<()> {
        let driver_idx = {
            let devices = self.devices.read().await;
            devices
                .get(device_id)
                .ok_or_else(|| SyscityError::NotFound {
                    resource: format!("Device '{}'", device_id),
                })?
                .driver_idx
        };

        // Remove old device entry (emits Disconnected event).
        if let Err(e) = self.disconnect(device_id).await {
            tracing::warn!("Reconnect: disconnect failed for '{}': {}", device_id, e);
        }

        // Connect fresh device through the same driver.
        let new_device = {
            let drivers = self.drivers.lock().await;
            drivers[driver_idx]
                .connect()
                .await
                .map_err(|e| SyscityError::ExternalService {
                    source: format!("Reconnect failed for '{}': {}", device_id, e),
                    cause: None,
                })?
        };
        let new_device = Arc::new(new_device);
        let new_id = new_device.id().to_string();

        // Insert fresh entry with the same driver index.
        {
            let mut devices = self.devices.write().await;
            devices.insert(
                new_id.clone(),
                DeviceEntry {
                    device: new_device.clone(),
                    driver_idx,
                },
            );
        }
        self.locks
            .write()
            .await
            .insert(new_id.clone(), Arc::new(RwLock::new(())));

        // Emit Connected event.
        self.emit_status_event(&new_id, DeviceStatus::Disconnected)
            .await;

        tracing::info!("Device reconnected: {} (new id: {})", device_id, new_id);
        Ok(())
    }

    /// Probe a single driver by name.
    ///
    /// Returns `Ok(true)` if the hardware is present, `Ok(false)` if absent.
    pub async fn probe_driver(&self, driver_name: &str) -> Result<bool> {
        let driver = {
            let drivers = self.drivers.lock().await;
            drivers
                .iter()
                .find(|d| d.driver_name() == driver_name)
                .ok_or_else(|| SyscityError::NotFound {
                    resource: format!("Device driver '{}'", driver_name),
                })?
                .clone()
        };
        driver.probe().await
    }

    /// Return driver names of currently connected devices.
    ///
    /// This is useful for hot-plug logic that compares all registered
    /// drivers against currently connected ones.
    pub async fn connected_driver_names(&self) -> Vec<String> {
        let devices = self.devices.read().await;
        let drivers = self.drivers.lock().await;
        devices
            .values()
            .map(|e| drivers[e.driver_idx].driver_name().to_string())
            .collect()
    }

    // ── Private helpers ─────────────────────────────────────────────────────

    /// Emit a [`DeviceStatusEvent`] on the status bus, reading the current
    /// status from the live device.
    async fn emit_status_event(&self, device_id: &str, previous: DeviceStatus) {
        let current = match self.get(device_id).await {
            Some(device) => device.status.read().await.clone(),
            None => DeviceStatus::Disconnected,
        };
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let _ = self.status_tx.send(DeviceStatusEvent {
            device_id: device_id.to_string(),
            previous,
            current,
            timestamp_millis: timestamp,
        });
    }

    /// Get the number of registered drivers.
    pub async fn driver_count(&self) -> usize {
        self.drivers.lock().await.len()
    }

    /// List registered driver names.
    pub async fn driver_names(&self) -> Vec<String> {
        let drivers = self.drivers.lock().await;
        drivers
            .iter()
            .map(|d| d.driver_name().to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use tokio::sync::broadcast::error::TryRecvError;

    use super::*;
    use crate::device::safety::{SafetyRule, SafetyRuleKind, SafetyZone};
    use crate::device::DeviceInfo;

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
        assert!(reg.driver_names().await.is_empty());
    }

    #[tokio::test]
    async fn test_probe_and_connect() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;
        reg.register(Arc::new(MockDriver {
            name: "camera".into(),
            present: false, // not present
        }))
        .await;

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
        }))
        .await;
        reg.register(Arc::new(MockDriver {
            name: "sensor-b".into(),
            present: true,
        }))
        .await;

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
        }))
        .await;
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
        }))
        .await;
        reg.register(Arc::new(MockDriver {
            name: "b".into(),
            present: true,
        }))
        .await;
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
        }))
        .await;
        reg.connect("motor").await.unwrap();

        // Mock driver health check always returns Ok(true)
        let healthy = reg.health_check("dev-motor").await.unwrap();
        assert!(healthy);
    }

    #[tokio::test]
    async fn test_driver_count_and_names() {
        let mut reg = DeviceRegistry::new();
        assert_eq!(reg.driver_count().await, 0);
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;
        reg.register(Arc::new(MockDriver {
            name: "camera".into(),
            present: false,
        }))
        .await;
        assert_eq!(reg.driver_count().await, 2);
        let names = reg.driver_names().await;
        assert!(names.contains(&"motor".into()));
        assert!(names.contains(&"camera".into()));
    }

    // ── Status bus tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_subscribe_status_connect() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;

        let mut rx = reg.subscribe_status();
        reg.connect("motor").await.unwrap();

        let event = rx.try_recv().expect("expected a status event");
        assert_eq!(event.device_id, "dev-motor");
        assert!(matches!(event.previous, DeviceStatus::Disconnected));
        assert!(matches!(event.current, DeviceStatus::Connected { .. }));
    }

    #[tokio::test]
    async fn test_subscribe_status_disconnect() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;

        reg.connect("motor").await.unwrap();

        let mut rx = reg.subscribe_status();
        reg.disconnect("dev-motor").await.unwrap();

        let event = rx.try_recv().expect("expected a status event");
        assert_eq!(event.device_id, "dev-motor");
        assert!(matches!(event.previous, DeviceStatus::Connected { .. }));
        assert!(matches!(event.current, DeviceStatus::Disconnected));
    }

    #[tokio::test]
    async fn test_subscribe_status_multiple_events() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;

        let mut rx = reg.subscribe_status();
        reg.connect("motor").await.unwrap();

        reg.set_device_status(
            "dev-motor",
            DeviceStatus::Degraded {
                message: "test".into(),
                since: crate::device::now_nanos(),
            },
        )
        .await;

        reg.disconnect("dev-motor").await.unwrap();

        let mut events = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(evt) => events.push(evt.current),
                Err(TryRecvError::Empty) => break,
                Err(e) => panic!("unexpected error: {:?}", e),
            }
        }

        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], DeviceStatus::Connected { .. }));
        assert!(matches!(events[1], DeviceStatus::Degraded { .. }));
        assert!(matches!(events[2], DeviceStatus::Disconnected));
    }

    // ── set_device_status tests ─────────────────────────────────────────

    #[tokio::test]
    async fn test_set_device_status_updates_and_emits() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;

        let mut rx = reg.subscribe_status();
        reg.connect("motor").await.unwrap();
        // drain the connect event — we are testing set_device_status only
        let _ = rx.try_recv();

        reg.set_device_status(
            "dev-motor",
            DeviceStatus::Error {
                message: "overheat".into(),
                since: 0,
            },
        )
        .await;

        let device = reg.get("dev-motor").await.unwrap();
        assert!(device.status.read().await.has_error());

        let event = rx.try_recv().expect("expected an error status event");
        assert!(matches!(event.current, DeviceStatus::Error { .. }));
    }

    #[tokio::test]
    async fn test_set_device_status_nonexistent_device() {
        let reg = DeviceRegistry::new();
        let mut rx = reg.subscribe_status();

        reg.set_device_status("ghost", DeviceStatus::Error { message: "x".into(), since: 0 })
            .await;

        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    // ── reconnect tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_reconnect_disconnects_and_reconnects() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;

        reg.connect("motor").await.unwrap();

        let mut rx = reg.subscribe_status();
        reg.reconnect("dev-motor").await.unwrap();

        assert_eq!(reg.len().await, 1);

        let event1 = rx.try_recv().expect("expected first status event");
        assert!(matches!(event1.current, DeviceStatus::Disconnected));

        let event2 = rx.try_recv().expect("expected second status event");
        assert!(matches!(event2.current, DeviceStatus::Connected { .. }));
    }

    #[tokio::test]
    async fn test_reconnect_nonexistent_device() {
        let reg = DeviceRegistry::new();
        let err = reg.reconnect("ghost").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── probe_driver tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_probe_driver_present() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;
        assert!(reg.probe_driver("motor").await.unwrap());
    }

    #[tokio::test]
    async fn test_probe_driver_absent() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "camera".into(),
            present: false,
        }))
        .await;
        assert!(!reg.probe_driver("camera").await.unwrap());
    }

    #[tokio::test]
    async fn test_probe_driver_unknown() {
        let reg = DeviceRegistry::new();
        let err = reg.probe_driver("nonexistent").await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    // ── connected_driver_names test ─────────────────────────────────────

    #[tokio::test]
    async fn test_connected_driver_names() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "a".into(),
            present: true,
        }))
        .await;
        reg.register(Arc::new(MockDriver {
            name: "b".into(),
            present: true,
        }))
        .await;
        reg.connect("a").await.unwrap();
        let names = reg.connected_driver_names().await;
        assert_eq!(names, vec!["a"]);
    }

    // ── try_lock tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_try_lock_success() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;
        reg.connect("motor").await.unwrap();

        let lock = reg.try_lock("dev-motor").await;
        assert!(lock.is_some());
        assert_eq!(lock.as_ref().unwrap().device().id(), "dev-motor");
        drop(lock);

        assert!(reg.try_lock("dev-motor").await.is_some());
    }

    #[tokio::test]
    async fn test_try_lock_contention() {
        let mut reg = DeviceRegistry::new();
        reg.register(Arc::new(MockDriver {
            name: "motor".into(),
            present: true,
        }))
        .await;
        reg.connect("motor").await.unwrap();

        let lock1 = reg.try_lock("dev-motor").await;
        assert!(lock1.is_some());

        let lock2 = reg.try_lock("dev-motor").await;
        assert!(lock2.is_none());

        drop(lock1);

        let lock3 = reg.try_lock("dev-motor").await;
        assert!(lock3.is_some());
    }
}
