//! Device subsystem initialization.
//!
//! Discovers, probes, and connects physical devices.  Each device capability
//! is wrapped as a [`DeviceToolWrapper`] and registered in [`ToolRegistry`] so
//! the LLM can discover and call device operations through standard tool
//! function calling — no Agent changes needed.
//!
//! # Health check loop
//!
//! When devices are connected, a background task periodically runs health
//! checks on all devices and logs warnings for degraded hardware.

use std::sync::Arc;

use crate::device::health::spawn_health_check_loop;
use crate::device::hotplug::spawn_hot_plug_loop;
use crate::device::os_bridge::bridge::{spawn_os_bridge_loop, DriverBuilder};
use crate::device::os_bridge::OsBridgeConfig;
use crate::device::registry::DeviceRegistry;
use crate::device::DeviceDriver;
use crate::device::DriverFactory;
use crate::gateway::DeviceConfig;
use crate::gateway::TaskRegistry;
use crate::perception::{DeviceSourceAdapter, PerceptionRegistry};
use crate::tools::device_tool::DeviceToolWrapper;
use crate::tools::ToolRegistry;

/// Device subsystem initialization result.
///
/// Background task handles are owned by the gateway's [`TaskRegistry`] under
/// the `device:` prefix; this struct only retains the device registry so that
/// callers can access discovered devices and trigger reloads.
pub struct DeviceInit {
    /// Registry managing all discovered devices and their lifecycle
    /// (health checks, reconnect, disconnect).
    pub registry: Arc<DeviceRegistry>,
}

/// Initialize the device subsystem.
///
/// 1. Returns early with `None` if `config.enabled` is `false`.
/// 2. Registers each driver in a fresh [`DeviceRegistry`].
/// 3. Probes all drivers to discover present hardware.
/// 4. Connects each present device.
/// 5. Wraps each capability as a [`DeviceToolWrapper`] and registers it in
///    `tool_registry` so the LLM can discover and call device operations.
/// 6. Spawns a background health-check loop.
///
/// Returns `None` when the device subsystem is disabled or no drivers
/// are provided.
pub async fn init_devices(
    config: &DeviceConfig,
    drivers: Vec<Arc<dyn DeviceDriver>>,
    tool_registry: &ToolRegistry,
    perception_registry: Option<&PerceptionRegistry>,
    task_registry: &crate::gateway::TaskRegistry,
) -> crate::Result<Option<DeviceInit>> {
    if !config.enabled || drivers.is_empty() {
        return Ok(None);
    }

    let device_registry = DeviceRegistry::new();

    for driver in drivers {
        device_registry.register(driver).await;
    }

    let present = device_registry.probe_all().await?;
    for driver_name in &present {
        let device = device_registry.connect(driver_name).await?;
        let device_id = device.id().to_string();

        // Register capabilities as tools in ToolRegistry so the LLM can call them.
        // Use register_dynamic since we only have &ToolRegistry (Arc-friendly).
        for cap in &device.capabilities {
            let wrapper = DeviceToolWrapper::new(driver_name.clone(), cap.clone());
            tool_registry.register_dynamic(Arc::new(wrapper));

            // Also register as a perception source if a registry is provided.
            if let Some(per_reg) = perception_registry {
                per_reg
                    .register_source(Arc::new(DeviceSourceAdapter::new(
                        device_id.clone(),
                        cap.clone(),
                    )))
                    .await;
            }
        }
    }

    let registry = Arc::new(device_registry);

    // Spawn background health-check loop
    if config.health_check.interval_secs > 0 {
        let reg = registry.clone();
        let cfg = config.health_check.clone();
        let handle = spawn_health_check_loop(reg, cfg);
        task_registry.insert_join("device:health", handle).await;
    }

    // Spawn background hot-plug detection loop
    if config.hot_plug.scan_interval_secs > 0 {
        let reg = registry.clone();
        let cfg = config.hot_plug.clone();
        let handle = spawn_hot_plug_loop(reg, cfg);
        task_registry.insert_join("device:hotplug", handle).await;
    }

    Ok(Some(DeviceInit { registry }))
}

// ── Config-driven driver discovery ──────────────────────────────────────

/// Convert [`DeviceDriverEntry`] items from config into driver instances.
///
/// Each entry is built via [`DriverFactory`].  Failures are logged as warnings
/// and skipped so that a single misconfigured entry does not block the entire
/// device subsystem.
pub fn discover_drivers_from_config(
    factory: &DriverFactory,
    config: &DeviceConfig,
) -> Vec<Arc<dyn DeviceDriver>> {
    let mut drivers = Vec::new();
    for entry in &config.drivers {
        match factory.build(&entry.kind, entry.params.clone()) {
            Ok(d) => drivers.push(d),
            Err(e) => {
                tracing::warn!("Failed to build device driver '{}': {}", entry.kind, e);
            }
        }
    }
    drivers
}

/// Spawn the OS device bridge loop from configuration settings.
///
/// Call this after `init_devices()` succeeds. Registers the bridge task in the
/// provided [`TaskRegistry`] under the `device:os_bridge` name when enabled.
pub async fn spawn_os_bridge_from_config(
    factory: &DriverFactory,
    registry: Arc<DeviceRegistry>,
    os_bridge: &OsBridgeConfig,
    tool_registry: Arc<ToolRegistry>,
    perception_registry: Option<Arc<PerceptionRegistry>>,
    task_registry: Arc<TaskRegistry>,
) {
    if !os_bridge.enabled || os_bridge.matchers.is_empty() {
        return;
    }

    let matchers = os_bridge.matchers.clone();
    let factory = factory.clone();
    let build_driver: DriverBuilder =
        Arc::new(move |kind: &str, params: serde_json::Value| factory.build(kind, params));

    let handle =
        spawn_os_bridge_loop(registry, matchers, tool_registry, perception_registry, build_driver);

    task_registry.insert_join("device:os_bridge", handle).await;
}

/// Reload the device subsystem from configuration.
///
/// 1. Disconnects all devices in the old registry.
/// 2. Aborts old device background tasks via the [`TaskRegistry`] (`device:`
///    prefix).
/// 3. Deregisters all old device tools from the [`ToolRegistry`].
/// 4. Re-runs driver discovery and init with the new `config`.
///
/// Returns the new [`DeviceInit`] (containing only the registry).
pub async fn reload_devices(
    old: DeviceInit,
    factory: &DriverFactory,
    config: &DeviceConfig,
    tool_registry: &ToolRegistry,
    perception_registry: Option<&PerceptionRegistry>,
    task_registry: Arc<TaskRegistry>,
) -> crate::Result<Option<DeviceInit>> {
    // 1. Disconnect all old devices
    old.registry.disconnect_all().await;

    // 2. Abort old device background tasks via the unified registry.
    task_registry.abort_matching("device:").await;

    // 3. Deregister all old device tools
    tool_registry.deregister_prefix("device_");

    // 3b. Deregister old device perception sources
    if let Some(per_reg) = perception_registry {
        per_reg.deregister_prefix("device:").await;
    }

    // 4. Re-run discovery and init
    let drivers = discover_drivers_from_config(factory, config);
    init_devices(config, drivers, tool_registry, perception_registry, &task_registry).await
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::device::mock::MockDeviceDriver;
    use crate::device::Capability;
    use crate::gateway::DeviceDriverEntry;

    struct DummySensor;

    #[async_trait]
    impl Capability for DummySensor {
        fn name(&self) -> &str {
            "sensor.read_temperature"
        }
        fn param_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }
        async fn execute(&self, _params: serde_json::Value) -> crate::device::CapabilityResult {
            crate::device::CapabilityResult {
                success: true,
                output: Some(json!({ "celsius": 23.5 })),
                error: None,
                duration_ms: 2,
            }
        }
    }

    #[tokio::test]
    async fn test_init_devices_disabled() {
        let config = DeviceConfig {
            enabled: false,
            ..Default::default()
        };
        let registry = TaskRegistry::new();
        let result = init_devices(&config, vec![], &ToolRegistry::new(), None, &registry)
            .await
            .expect("init_devices should succeed when disabled");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_init_devices_with_mock_drivers() {
        let sensor = Arc::new(DummySensor);
        let driver = MockDeviceDriver::new("sensor-01", true)
            .with_capabilities(vec![sensor as Arc<dyn Capability>]);

        let config = DeviceConfig::default();
        let tool_registry = ToolRegistry::new();
        let task_registry = TaskRegistry::new();

        let result =
            init_devices(&config, vec![Arc::new(driver)], &tool_registry, None, &task_registry)
                .await
                .expect("init_devices should succeed")
                .expect("init_devices should return Some when enabled");

        // Verify the device is registered
        assert_eq!(result.registry.len().await, 1);

        // Verify tool is registered in ToolRegistry
        let names = tool_registry.list();
        assert!(
            names.contains(&"device_sensor-01_sensor_read_temperature".to_string()),
            "tool should be registered in ToolRegistry: {:?}",
            names,
        );

        // Verify background tasks were registered
        assert!(task_registry.contains("device:health").await);
        assert!(task_registry.contains("device:hotplug").await);
    }

    #[tokio::test]
    async fn test_init_devices_empty_drivers() {
        let config = DeviceConfig::default();
        let registry = TaskRegistry::new();
        let result = init_devices(&config, vec![], &ToolRegistry::new(), None, &registry)
            .await
            .expect("init_devices should succeed with no drivers");
        assert!(result.is_none());
    }

    // ── discover_drivers_from_config tests ───────────────────────────

    #[test]
    fn test_discover_drivers_from_config_empty() {
        let factory = DriverFactory::new();
        let config = DeviceConfig::default(); // no drivers
        let drivers = discover_drivers_from_config(&factory, &config);
        assert!(drivers.is_empty());
    }

    #[test]
    fn test_discover_drivers_from_config_with_mock() {
        let factory = DriverFactory::new();
        let config = DeviceConfig {
            drivers: vec![DeviceDriverEntry {
                kind: "mock".into(),
                params: json!({ "name": "sensor-01", "present": true }),
            }],
            ..Default::default()
        };
        let drivers = discover_drivers_from_config(&factory, &config);
        assert_eq!(drivers.len(), 1);
        assert_eq!(drivers[0].driver_name(), "sensor-01");
    }

    #[test]
    fn test_discover_drivers_from_config_skips_unknown() {
        let factory = DriverFactory::new();
        let config = DeviceConfig {
            drivers: vec![
                DeviceDriverEntry {
                    kind: "mock".into(),
                    params: json!({ "name": "good" }),
                },
                DeviceDriverEntry {
                    kind: "nosuchdriver".into(),
                    params: json!({}),
                },
            ],
            ..Default::default()
        };
        let drivers = discover_drivers_from_config(&factory, &config);
        assert_eq!(drivers.len(), 1);
        assert_eq!(drivers[0].driver_name(), "good");
    }
}
