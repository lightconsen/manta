//! Control lane — high-priority parallel execution channel for safety
//! monitoring and fast-path device actions.
//!
//! The control lane runs on a **separate single-threaded tokio runtime**,
//! independent of the main agent runtime.  It periodically performs health
//! checks on all devices and invokes registered [`ControlHandler`] callbacks.
//!
//! # Architecture
//!
//! ```text
//! Agent Runtime (multi-threaded)          Control Runtime (single-threaded)
//! ┌─────────────────────────────┐         ┌──────────────────────────────┐
//! │ Agent Loop, Health Check,   │         │ control_loop tick every 50ms │
//! │ Hotplug, OS Bridge          │         │ 1. health_check_all()        │
//! │                             │         │ 2. check device statuses     │
//! │ DeviceRegistry (Arc) ◄──────┼─────────┼─► read/update                │
//! │ SafetyZone.engaged (Arc) ◄──┼─────────┼─► fast_trip() on fault       │
//! └─────────────────────────────┘         └──────────────────────────────┘
//! ```
//!
//! The control lane is **optional** (disabled by default).  When disabled,
//! no runtime is created and no overhead is incurred.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::device::registry::DeviceRegistry;
use crate::device::Device;

// ── Public types
// ──────────────────────────────────────────────────────────────

/// A callback invoked by the control loop for a specific device.
///
/// Handlers receive an [`Arc<Device>`] and can read its status, inspect its
/// [`SafetyZone`], or perform fast-path actions.  The handler runs on the
/// **control runtime** — it must not block for more than a few milliseconds.
pub type ControlHandler = Box<dyn Fn(Arc<Device>) + Send + Sync>;

/// Thread-safe registry of [`ControlHandler`] callbacks, keyed by device ID.
pub type ControlHandlerRegistry = Arc<Mutex<HashMap<String, Vec<ControlHandler>>>>;

/// Configuration for the control lane.
///
/// # Defaults
///
/// | Field              | Default | Description                                  |
/// |--------------------|---------|----------------------------------------------|
/// | `enabled`          | false   | Enable the separate control runtime          |
/// | `loop_interval_ms` | 50      | Tick interval (ms) for the control loop      |
/// | `pin_cpu`          | false   | Pin control thread to a dedicated CPU (Linux)|
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlConfig {
    /// Enable the control lane.  When `false`, no runtime or loop is created.
    #[serde(default)]
    pub enabled: bool,
    /// Tick interval in milliseconds for the control loop (default 50).
    #[serde(default = "default_loop_interval")]
    pub loop_interval_ms: u64,
    /// Pin the control thread to a dedicated CPU core (Linux only, requires
    /// CAP_SYS_NICE or root).
    #[cfg(target_os = "linux")]
    #[serde(default)]
    pub pin_cpu: bool,
}

const fn default_loop_interval() -> u64 {
    50
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            loop_interval_ms: default_loop_interval(),
            #[cfg(target_os = "linux")]
            pin_cpu: false,
        }
    }
}

// ── Control loop (runs on existing runtime) ─────────────────────────────────

/// Spawn the control loop on the **current** tokio runtime.
///
/// The loop ticks every `config.loop_interval_ms` and:
/// 1. Calls [`DeviceRegistry::health_check_all`].
/// 2. For each device in `Error` state: updates its [`SafetyZone::engaged`] to
///    `true` (fast-trip).
/// 3. Invokes registered [`ControlHandler`] callbacks for each device.
///
/// Returns a [`JoinHandle`] that can be aborted during shutdown / reload.
pub fn spawn_control_loop(
    registry: Arc<DeviceRegistry>,
    handlers: ControlHandlerRegistry,
    config: ControlConfig,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval = Duration::from_millis(config.loop_interval_ms);
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            ticker.tick().await;

            // 1. Run health checks on all devices.
            let health_results = registry.health_check_all().await;

            // 2. Fast-trip devices in Error state.
            for (id, healthy) in &health_results {
                if !healthy {
                    if let Some(device) = registry.get(id).await {
                        // Fast-trip the safety zone.
                        let sz = device.safety_zone.read().await;
                        sz.fast_trip();
                        drop(sz);

                        tracing::warn!(
                            "Control lane: device '{}' health check failed, safety engaged",
                            id,
                        );
                    }
                }
            }

            // 3. Invoke registered control handlers per device.
            let device_ids: Vec<String> = health_results.keys().cloned().collect();
            for id in &device_ids {
                if let Some(device) = registry.get(id).await {
                    // Hold the lock only while iterating handlers (synchronous).
                    let map = handlers.lock().unwrap();
                    if let Some(handler_list) = map.get(id) {
                        for handler in handler_list {
                            handler(device.clone());
                        }
                    }
                }
            }
        }
    })
}

// ── Dedicated control runtime ───────────────────────────────────────────────

/// Create a dedicated **single-threaded** tokio runtime and spawn the
/// control loop on it.
///
/// Returns `(Runtime, JoinHandle<()>)`.  The caller must keep the
/// [`Runtime`] alive — dropping it will stop all tasks running on it.
///
/// # Linux CPU Pinning
///
/// When `config.pin_cpu` is `true` and the platform is Linux, the builder
/// sets [`tokio::runtime::Builder::on_thread_start`] to pin the control
/// thread to CPU core 3 (arbitrary; tune for your hardware).
pub fn init_control_runtime(
    registry: Arc<DeviceRegistry>,
    handlers: ControlHandlerRegistry,
    config: ControlConfig,
) -> (Runtime, JoinHandle<()>) {
    let mut builder = tokio::runtime::Builder::new_current_thread();
    builder.enable_all();
    builder.thread_name("control-lane");

    #[cfg(target_os = "linux")]
    if config.pin_cpu {
        builder.on_thread_start(|| {
            // Pin to CPU core 3 (adjust for your hardware topology).
            // Requires CAP_SYS_NICE or root.
            unsafe {
                let mut cpu_set: libc::cpu_set_t = std::mem::zeroed();
                libc::CPU_SET(3, &mut cpu_set);
                let ret =
                    libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &cpu_set);
                if ret != 0 {
                    tracing::warn!("Control lane: failed to pin thread to CPU 3");
                }
            }
        });
    }

    let rt = builder.build().expect("control lane runtime");
    // The async block intentionally returns the JoinHandle without awaiting it.
    #[allow(clippy::async_yields_async)]
    let handle = rt.block_on(async move { spawn_control_loop(registry, handlers, config) });

    (rt, handle)
}

// ── Helper to create an empty handler registry ──────────────────────────────

/// Create a new empty [`ControlHandlerRegistry`].
pub fn new_handler_registry() -> ControlHandlerRegistry {
    Arc::new(Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use tokio::time::Duration;

    use super::*;
    use crate::device::safety::{SafetyRule, SafetyRuleKind};
    use crate::device::{Device, DeviceDriver, DeviceInfo, SafetyZone};

    struct TestDriver {
        name: String,
        healthy: bool,
    }

    #[async_trait]
    impl DeviceDriver for TestDriver {
        fn driver_name(&self) -> &str {
            &self.name
        }

        async fn probe(&self) -> crate::error::Result<bool> {
            Ok(true)
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
            Ok(self.healthy)
        }
    }

    async fn make_registry() -> Arc<DeviceRegistry> {
        let reg = DeviceRegistry::new();
        let driver = TestDriver {
            name: "sensor-01".into(),
            healthy: false,
        };
        reg.register(Arc::new(driver)).await;
        let _ = reg.connect("sensor-01").await;
        Arc::new(reg)
    }

    #[tokio::test]
    async fn test_spawn_control_loop_runs() {
        let registry = make_registry().await;
        let handlers = new_handler_registry();

        let handle = spawn_control_loop(
            registry.clone(),
            handlers,
            ControlConfig {
                enabled: true,
                loop_interval_ms: 20,
                ..Default::default()
            },
        );

        // Let the loop tick a couple of times
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The device should have been fast-tripped (Error → safety engaged)
        let device = registry.get("dev-sensor-01").await.unwrap();
        let sz = device.safety_zone.read().await;
        assert!(sz.is_engaged(), "safety should be engaged for unhealthy device");
        drop(sz);

        handle.abort();
    }

    #[tokio::test]
    async fn test_control_handler_invoked() {
        let registry = make_registry().await;
        let handlers = new_handler_registry();
        let invoked = Arc::new(AtomicBool::new(false));

        let inv = invoked.clone();
        handlers.lock().unwrap().insert(
            "dev-sensor-01".into(),
            vec![Box::new(move |_device| {
                inv.store(true, Ordering::Release);
            })],
        );

        let handle = spawn_control_loop(
            registry.clone(),
            handlers,
            ControlConfig {
                enabled: true,
                loop_interval_ms: 20,
                ..Default::default()
            },
        );

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(invoked.load(Ordering::Acquire), "control handler should have been invoked");
        handle.abort();
    }

    #[test]
    fn test_init_control_runtime_creates_runtime() {
        // Create a separate runtime (outside tokio::test) and keep it alive
        // on a background thread so the control loop can tick.
        let rt = tokio::runtime::Runtime::new().unwrap();

        let registry = rt.block_on(make_registry());
        let handlers = new_handler_registry();

        let (ctrl_rt, handle) = init_control_runtime(
            registry.clone(),
            handlers,
            ControlConfig {
                enabled: true,
                loop_interval_ms: 50,
                ..Default::default()
            },
        );

        // Keep the control runtime alive on a background thread.
        std::thread::spawn(move || {
            // Use a oneshot channel receiver that never resolves to
            // keep block_on running (polls spawned tasks).
            let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
            let _ = ctrl_rt.block_on(rx);
        });

        // Give the loop enough time for several ticks
        std::thread::sleep(Duration::from_millis(200));

        let device = rt.block_on(async { registry.get("dev-sensor-01").await.unwrap() });
        let sz = rt.block_on(async { device.safety_zone.read().await });
        assert!(sz.is_engaged(), "control loop should have tripped safety for unhealthy device");
        drop(sz);

        handle.abort();
    }
}
