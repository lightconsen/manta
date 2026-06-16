//! Hot-plug detection loop — periodically re-probes absent drivers.
//!
//! When a previously absent driver reports its hardware present, the loop
//! auto-connects the device and emits a [`DeviceStatusEvent`] so consumers
//! (TUI, WebSocket, Agent) can react in real time.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::device::registry::DeviceRegistry;

/// Configuration for the background hot-plug detection loop.
///
/// # Defaults
///
/// | Field               | Default | Description                                  |
/// |---------------------|---------|----------------------------------------------|
/// | `scan_interval_secs`| 10      | How often to re-probe absent drivers         |
/// | `auto_connect`      | true    | Whether to auto-connect newly found devices  |
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotPlugConfig {
    /// Interval (seconds) between probe scans of absent drivers.
    #[serde(default = "default_scan_interval")]
    pub scan_interval_secs: u64,
    /// When `true`, newly discovered devices are connected automatically.
    #[serde(default = "default_auto_connect")]
    pub auto_connect: bool,
}

const fn default_scan_interval() -> u64 { 10 }
const fn default_auto_connect() -> bool { true }

impl Default for HotPlugConfig {
    fn default() -> Self {
        Self {
            scan_interval_secs: default_scan_interval(),
            auto_connect: default_auto_connect(),
        }
    }
}

/// Spawn a background task that periodically re-probes absent device drivers.
///
/// The loop:
/// 1. Lists all registered drivers and determines which are not connected.
/// 2. Re-probes each absent driver.
/// 3. If a driver now reports the device present, optionally auto-connects.
/// 4. Emits [`DeviceStatusEvent`]s through the registry's status bus.
pub fn spawn_hot_plug_loop(
    registry: Arc<DeviceRegistry>,
    config: HotPlugConfig,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker =
            tokio::time::interval(Duration::from_secs(config.scan_interval_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            ticker.tick().await;

            let all = registry.driver_names();
            let connected = registry.connected_driver_names().await;

            for driver_name in &all {
                if connected.contains(driver_name) {
                    continue; // already connected, nothing to do
                }

                // Re-probe the absent driver.
                match registry.probe_driver(driver_name).await {
                    Ok(true) => {
                        tracing::info!(
                            "Hot-plug detected: driver '{}' now present",
                            driver_name
                        );
                        if config.auto_connect {
                            match registry.connect(driver_name).await {
                                Ok(device) => {
                                    tracing::info!(
                                        "Auto-connected hot-plug device: {} ({})",
                                        device.id(),
                                        device.model()
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Hot-plug connect failed for '{}': {}",
                                        driver_name,
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Ok(false) => { /* hardware still absent */ }
                    Err(e) => {
                        tracing::debug!(
                            "Hot-plug probe error for '{}': {}",
                            driver_name,
                            e
                        );
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use async_trait::async_trait;
    use tokio::sync::broadcast::error::TryRecvError;

    use crate::device::DeviceDriver;

    use crate::device::{Device, DeviceInfo, DeviceStatus};
    use crate::device::mock::MockDeviceDriver;
    use crate::device::safety::{SafetyRule, SafetyRuleKind, SafetyZone};

    struct ToggleProbeDriver {
        name: String,
        present: Arc<AtomicBool>,
    }

    impl ToggleProbeDriver {
        fn new(name: &str, present: bool) -> (Self, Arc<AtomicBool>) {
            let p = Arc::new(AtomicBool::new(present));
            (Self {
                name: name.into(),
                present: p.clone(),
            }, p)
        }
    }

    #[async_trait]
    impl DeviceDriver for ToggleProbeDriver {
        fn driver_name(&self) -> &str {
            &self.name
        }

        async fn probe(&self) -> crate::error::Result<bool> {
            Ok(self.present.load(Ordering::Acquire))
        }

        async fn connect(&self) -> crate::error::Result<Device> {
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

        async fn health_check(&self) -> crate::error::Result<bool> {
            Ok(true)
        }
    }

    #[tokio::test]
    async fn test_hot_plug_absent_becomes_present() {
        let mut reg = DeviceRegistry::new();
        let (driver, present) = ToggleProbeDriver::new("sensor-01", false);
        reg.register(Arc::new(driver));
        let registry = Arc::new(reg);
        let mut rx = registry.subscribe_status();

        let handle = spawn_hot_plug_loop(
            registry.clone(),
            HotPlugConfig {
                scan_interval_secs: 1,
                auto_connect: true,
            },
        );

        // Let 1 tick pass — nothing should happen since probe=false
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(registry.is_empty().await);

        // Hardware appears
        present.store(true, Ordering::Release);

        // Wait for a Connected event
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("should receive Connected event within 5s")
            .expect("event should not be lagged");

        assert!(
            matches!(event.current, DeviceStatus::Connected { .. }),
            "expected Connected event, got {:?}",
            event.current
        );

        let ids = registry.list().await;
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], "dev-sensor-01");

        let _ = handle.abort();
    }

    #[tokio::test]
    async fn test_hot_plug_skips_connected_driver() {
        let mut reg = DeviceRegistry::new();
        let (driver, _present) = ToggleProbeDriver::new("sensor-01", true);
        reg.register(Arc::new(driver));
        let registry = Arc::new(reg);

        // Manually connect — this emits a Connected event
        registry.connect("sensor-01").await.unwrap();
        assert_eq!(registry.len().await, 1);

        // Subscribe AFTER manual connect so we don't see that event
        let mut rx = registry.subscribe_status();

        let handle = spawn_hot_plug_loop(
            registry.clone(),
            HotPlugConfig {
                scan_interval_secs: 1,
                auto_connect: true,
            },
        );

        // Let 2 ticks pass — loop skips already-connected driver
        tokio::time::sleep(Duration::from_millis(2500)).await;

        // Still exactly 1 device (no duplicate connection)
        assert_eq!(registry.len().await, 1);

        // No additional Connected events received
        match rx.try_recv() {
            Err(TryRecvError::Empty) => {} // expected
            other => panic!("expected no additional events, got: {:?}", other),
        }

        let _ = handle.abort();
    }

    #[tokio::test]
    async fn test_hot_plug_auto_connect_false() {
        let mut reg = DeviceRegistry::new();
        let (driver, present) = ToggleProbeDriver::new("sensor-01", false);
        reg.register(Arc::new(driver));
        let registry = Arc::new(reg);

        let handle = spawn_hot_plug_loop(
            registry.clone(),
            HotPlugConfig {
                scan_interval_secs: 1,
                auto_connect: false,
            },
        );

        // 1 tick — nothing happens since probe=false
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(registry.is_empty().await);

        // Hardware appears
        present.store(true, Ordering::Release);

        // 2 more ticks — probe=true but auto_connect=false
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(registry.is_empty().await);

        // Probe driver should still report present
        assert!(registry.probe_driver("sensor-01").await.unwrap());

        let _ = handle.abort();
    }

    #[tokio::test]
    async fn test_hot_plug_connect_error_handled() {
        let mut reg = DeviceRegistry::new();
        let driver = MockDeviceDriver::new("sensor-01", true)
            .with_connect_error("connection refused");
        reg.register(Arc::new(driver));
        let registry = Arc::new(reg);

        let handle = spawn_hot_plug_loop(
            registry.clone(),
            HotPlugConfig {
                scan_interval_secs: 1,
                auto_connect: true,
            },
        );

        // Let 2 ticks pass — connect fails, loop logs warning and continues
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(registry.is_empty().await);

        let _ = handle.abort();
    }
}
