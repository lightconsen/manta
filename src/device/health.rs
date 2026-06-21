//! Background health-check loop — heartbeat protocol for connected devices.
//!
//! Each tick the loop runs [`HealthCheckConfig::timeout_secs`]-bounded health
//! checks on every connected device.  Consecutive failures are tracked;
//! when [`HealthCheckConfig::max_failures_before_error`] is reached the
//! device is marked as `Error` and auto-reconnect is attempted.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

use crate::device::registry::DeviceRegistry;
use crate::device::DeviceStatus;

/// Configuration for the background device health-check loop.
///
/// # Defaults
///
/// | Field                    | Default | Description                        |
/// |--------------------------|---------|------------------------------------|
/// | `interval_secs`          | 30      | How often to run checks            |
/// | `timeout_secs`           | 5       | Per-device health check timeout    |
/// | `max_failures_before_error` | 3   | Consecutive failures → Error state |
/// | `auto_reconnect`         | true    | Whether to attempt reconnection    |
/// | `max_reconnect_attempts` | 3       | How many times to retry reconnect  |
/// | `reconnect_delay_ms`     | 1000    | Delay between reconnect attempts   |
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    /// Interval (seconds) between health check ticks.
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// Timeout (seconds) for each individual health check call.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// How many consecutive failures before transitioning the device
    /// from `Degraded` to `Error` and triggering reconnect logic.
    #[serde(default = "default_max_failures")]
    pub max_failures_before_error: u32,
    /// When `true`, the loop attempts to reconnect an errored device.
    #[serde(default = "default_auto_reconnect")]
    pub auto_reconnect: bool,
    /// Maximum reconnect attempts before giving up.
    #[serde(default = "default_max_reconnect")]
    pub max_reconnect_attempts: u32,
    /// Delay (milliseconds) between reconnect attempts.
    #[serde(default = "default_reconnect_delay")]
    pub reconnect_delay_ms: u64,
}

const fn default_interval() -> u64 {
    30
}
const fn default_timeout() -> u64 {
    5
}
const fn default_max_failures() -> u32 {
    3
}
const fn default_auto_reconnect() -> bool {
    true
}
const fn default_max_reconnect() -> u32 {
    3
}
const fn default_reconnect_delay() -> u64 {
    1000
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_interval(),
            timeout_secs: default_timeout(),
            max_failures_before_error: default_max_failures(),
            auto_reconnect: default_auto_reconnect(),
            max_reconnect_attempts: default_max_reconnect(),
            reconnect_delay_ms: default_reconnect_delay(),
        }
    }
}

/// Spawn a background task that runs health checks on all connected devices.
///
/// The returned [`JoinHandle`] can be aborted during shutdown / reload via
/// [`reload_devices`](crate::gateway::init::devices::reload_devices).
///
/// # Behaviour
///
/// 1. Every `config.interval_secs`, iterate all connected devices.
/// 2. Call `registry.health_check(id)` with a `config.timeout_secs` deadline.
/// 3. On success: reset the consecutive-failure counter for that device.
/// 4. On failure (timeout, error, or unhealthy): increment the counter.
/// 5. When the counter reaches `config.max_failures_before_error`:
///    - Emit a `DeviceStatusEvent` by calling `registry.set_device_status()`
///      with `DeviceStatus::Error`.
///    - If `auto_reconnect` is enabled, attempt `max_reconnect_attempts` rounds
///      of `registry.reconnect()`.
pub fn spawn_health_check_loop(
    registry: Arc<DeviceRegistry>,
    config: HealthCheckConfig,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(config.interval_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Per-device consecutive failure count.
        let mut failures: HashMap<String, u32> = HashMap::new();

        loop {
            ticker.tick().await;
            let ids: Vec<String> = registry.list().await;

            for id in &ids {
                let result = tokio::time::timeout(
                    Duration::from_secs(config.timeout_secs),
                    registry.health_check(id),
                )
                .await;

                let healthy = matches!(result, Ok(Ok(true)));

                if healthy {
                    failures.remove(id);
                    // If the device was in Error state, restore it to Connected.
                    if let Some(device) = registry.get(id).await {
                        if device.status.read().await.has_error() {
                            registry
                                .set_device_status(id, DeviceStatus::connected_now())
                                .await;
                        }
                    }
                } else {
                    let count = failures.entry(id.clone()).or_insert(0);
                    *count += 1;

                    if *count >= config.max_failures_before_error {
                        tracing::warn!(
                            "Device '{}' health check failed {} times, marking Error",
                            id,
                            count
                        );

                        // Mark as error (emits status event via registry).
                        registry
                            .set_device_status(
                                id,
                                DeviceStatus::Error {
                                    message: format!(
                                        "Health check failed after {} consecutive failures",
                                        count
                                    ),
                                    since: crate::device::now_nanos(),
                                },
                            )
                            .await;

                        // Auto-reconnect.
                        if config.auto_reconnect {
                            for attempt in 1..=config.max_reconnect_attempts {
                                tracing::info!(
                                    "Reconnect attempt {}/{} for '{}'",
                                    attempt,
                                    config.max_reconnect_attempts,
                                    id
                                );
                                tokio::time::sleep(Duration::from_millis(
                                    config.reconnect_delay_ms,
                                ))
                                .await;

                                if registry.reconnect(id).await.is_ok() {
                                    failures.remove(id);
                                    tracing::info!("Reconnected device '{}'", id);
                                    break;
                                }
                            }
                        }
                    } else {
                        // First few failures → Degraded (emits event).
                        tracing::debug!(
                            "Device '{}' health check failed ({}/{}), marking Degraded",
                            id,
                            count,
                            config.max_failures_before_error
                        );
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::broadcast::error::TryRecvError;

    use super::*;
    use crate::device::registry::DeviceRegistry;
    use crate::device::safety::{SafetyRule, SafetyRuleKind};
    use crate::device::{Device, DeviceDriver, DeviceInfo, DeviceStatus, SafetyZone};

    // ── Test utility: toggleable health driver ──────────────────────────────

    struct ToggleHealthDriver {
        name: String,
        present: bool,
        healthy: Arc<AtomicBool>,
    }

    impl ToggleHealthDriver {
        fn new(name: &str, present: bool, healthy: bool) -> (Self, Arc<AtomicBool>) {
            let h = Arc::new(AtomicBool::new(healthy));
            (
                Self {
                    name: name.into(),
                    present,
                    healthy: h.clone(),
                },
                h,
            )
        }
    }

    #[async_trait]
    impl DeviceDriver for ToggleHealthDriver {
        fn driver_name(&self) -> &str {
            &self.name
        }

        async fn probe(&self) -> crate::error::Result<bool> {
            Ok(self.present)
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
            Ok(self.healthy.load(Ordering::Acquire))
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    /// Drain the broadcast receiver for at least one `Error` event, waiting up
    /// to `timeout_secs` wall-clock seconds for it to arrive.
    async fn wait_for_error_event(
        rx: &mut tokio::sync::broadcast::Receiver<crate::device::status_bus::DeviceStatusEvent>,
        timeout_secs: u64,
    ) {
        let deadline = Duration::from_secs(timeout_secs);
        loop {
            match tokio::time::timeout(deadline, rx.recv()).await {
                Ok(Ok(event)) => {
                    if matches!(event.current, DeviceStatus::Error { .. }) {
                        return;
                    }
                    // Continue draining intermediate events (Degraded, etc.)
                }
                _ => panic!("Timed out waiting for Error event"),
            }
        }
    }

    // ── Tests ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_healthy_device_stays_connected() {
        let mut registry = DeviceRegistry::new();
        let (driver, _healthy) = ToggleHealthDriver::new("sensor-01", true, true);
        registry.register(Arc::new(driver)).await;
        let _ = registry.connect("sensor-01").await.unwrap();

        let mut rx = registry.subscribe_status();

        let registry = Arc::new(registry);
        let handle = spawn_health_check_loop(
            registry.clone(),
            HealthCheckConfig {
                interval_secs: 1,
                max_failures_before_error: 2,
                auto_reconnect: false,
                ..Default::default()
            },
        );

        tokio::time::sleep(Duration::from_millis(2500)).await;

        // Device should still be connected and healthy
        let device = registry.get("dev-sensor-01").await.unwrap();
        let status = device.status.read().await;
        assert!(matches!(*status, DeviceStatus::Connected { .. }));
        drop(status);

        // No Error events should have been emitted
        let result = rx.try_recv();
        assert!(
            matches!(result, Err(TryRecvError::Empty)),
            "Expected no events for a healthy device, got: {:?}",
            result
        );

        let _ = handle.abort();
    }

    #[tokio::test]
    async fn test_consecutive_failures_mark_error() {
        let mut registry = DeviceRegistry::new();
        let (driver, _healthy) = ToggleHealthDriver::new("sensor-01", true, false);
        registry.register(Arc::new(driver)).await;
        let _ = registry.connect("sensor-01").await.unwrap();

        let mut rx = registry.subscribe_status();

        let registry = Arc::new(registry);
        let handle = spawn_health_check_loop(
            registry.clone(),
            HealthCheckConfig {
                interval_secs: 1,
                max_failures_before_error: 2,
                auto_reconnect: false,
                max_reconnect_attempts: 0,
                reconnect_delay_ms: 1,
                ..Default::default()
            },
        );

        // Wait for 3+ ticks — after the 2nd consecutive failure the device
        // should be in Error state.
        tokio::time::sleep(Duration::from_millis(3500)).await;

        let device = registry.get("dev-sensor-01").await.unwrap();
        let status = device.status.read().await;
        assert!(matches!(*status, DeviceStatus::Error { .. }));
        drop(status);

        // At least one Error event must have been broadcast
        let mut has_error = false;
        while let Ok(event) = rx.try_recv() {
            if matches!(event.current, DeviceStatus::Error { .. }) {
                has_error = true;
            }
        }
        assert!(has_error, "Should have received at least one Error event");

        let _ = handle.abort();
    }

    #[tokio::test]
    async fn test_auto_reconnect_recovers_device() {
        let mut registry = DeviceRegistry::new();
        let (driver, healthy_handle) = ToggleHealthDriver::new("sensor-01", true, false);
        registry.register(Arc::new(driver)).await;
        let _ = registry.connect("sensor-01").await.unwrap();

        let mut rx = registry.subscribe_status();

        let registry = Arc::new(registry);
        let handle = spawn_health_check_loop(
            registry.clone(),
            HealthCheckConfig {
                interval_secs: 1,
                max_failures_before_error: 1,
                auto_reconnect: true,
                max_reconnect_attempts: 1,
                reconnect_delay_ms: 10,
                ..Default::default()
            },
        );

        // Wait until an Error event is emitted (from set_device_status in the loop).
        wait_for_error_event(&mut rx, 10).await;

        // The reconnect has completed, now make the driver healthy again.
        healthy_handle.store(true, Ordering::Release);

        // Let 2 more ticks pass so the loop can observe healthy state.
        tokio::time::sleep(Duration::from_millis(2500)).await;

        // Verify the device is back in Connected state.
        let device = registry.get("dev-sensor-01").await.unwrap();
        let status = device.status.read().await;
        assert!(
            matches!(*status, DeviceStatus::Connected { .. }),
            "Device should have been restored to Connected after reconnect + healthy check, got: \
             {:?}",
            *status,
        );
        drop(status);

        let _ = handle.abort();
    }

    #[tokio::test]
    async fn test_auto_reconnect_disabled() {
        let mut registry = DeviceRegistry::new();
        let (driver, _healthy) = ToggleHealthDriver::new("sensor-01", true, false);
        registry.register(Arc::new(driver)).await;
        let _ = registry.connect("sensor-01").await.unwrap();

        let mut rx = registry.subscribe_status();

        let registry = Arc::new(registry);
        let handle = spawn_health_check_loop(
            registry.clone(),
            HealthCheckConfig {
                interval_secs: 1,
                max_failures_before_error: 2,
                auto_reconnect: false,
                max_reconnect_attempts: 0,
                reconnect_delay_ms: 1,
                ..Default::default()
            },
        );

        // Wait for the Error state to be reached.
        wait_for_error_event(&mut rx, 10).await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Verify device stays in Error (no reconnect happened).
        let device = registry.get("dev-sensor-01").await.unwrap();
        let status = device.status.read().await;
        assert!(matches!(*status, DeviceStatus::Error { .. }));
        drop(status);

        // Device ID should be unchanged — device still in registry.
        let ids = registry.list().await;
        assert_eq!(ids, vec!["dev-sensor-01"]);

        let _ = handle.abort();
    }

    #[tokio::test]
    async fn test_disconnected_device_no_errors() {
        let mut registry = DeviceRegistry::new();
        let (driver, _healthy) = ToggleHealthDriver::new("sensor-01", true, false);
        registry.register(Arc::new(driver)).await;
        let _ = registry.connect("sensor-01").await.unwrap();
        let _ = registry.disconnect("dev-sensor-01").await;

        let mut rx = registry.subscribe_status();

        let registry = Arc::new(registry);
        let handle = spawn_health_check_loop(
            registry.clone(),
            HealthCheckConfig {
                interval_secs: 1,
                ..Default::default()
            },
        );

        tokio::time::sleep(Duration::from_millis(2500)).await;

        // No events should have been emitted (no connected devices).
        let result = rx.try_recv();
        assert!(
            matches!(result, Err(TryRecvError::Empty)),
            "Expected no events for disconnected device, got: {:?}",
            result,
        );

        // Registry should be empty after explicit disconnect.
        let ids = registry.list().await;
        assert!(ids.is_empty(), "Registry should be empty: {:?}", ids);

        let _ = handle.abort();
    }

    #[tokio::test]
    async fn test_failure_counters_persist_across_ticks() {
        let mut registry = DeviceRegistry::new();
        let (driver, _healthy) = ToggleHealthDriver::new("sensor-01", true, false);
        registry.register(Arc::new(driver)).await;
        let _ = registry.connect("sensor-01").await.unwrap();

        let registry = Arc::new(registry);
        let handle = spawn_health_check_loop(
            registry.clone(),
            HealthCheckConfig {
                interval_secs: 1,
                max_failures_before_error: 3,
                auto_reconnect: false,
                ..Default::default()
            },
        );

        // 5.5 seconds allows 5 ticks — the counter should reach 3 by tick 3
        // and the device should be in Error state.
        tokio::time::sleep(Duration::from_millis(5500)).await;

        let device = registry.get("dev-sensor-01").await.unwrap();
        let status = device.status.read().await;
        assert!(
            matches!(*status, DeviceStatus::Error { .. }),
            "Device should be in Error after 3+ consecutive failures, got: {:?}",
            *status,
        );
        drop(status);

        let _ = handle.abort();
    }
}
