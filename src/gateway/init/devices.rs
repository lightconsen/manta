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

use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;

use crate::device::health::spawn_health_check_loop;
use crate::device::hotplug::spawn_hot_plug_loop;
use crate::device::mock::MockDeviceDriver;
use crate::device::os_bridge::bridge::{spawn_os_bridge_loop, DriverBuilder};
use crate::device::os_bridge::OsBridgeConfig;
use crate::device::registry::DeviceRegistry;
use crate::device::DeviceDriver;
use crate::error::SyscityError;
use crate::gateway::DeviceConfig;
use crate::tools::device_tool::DeviceToolWrapper;
use crate::tools::ToolRegistry;
use serde_json::Value;

/// Device subsystem initialization result.
pub struct DeviceInit {
    /// Registry managing all discovered devices and their lifecycle
    /// (health checks, reconnect, disconnect).
    pub registry: Arc<DeviceRegistry>,
    /// Background health-check loop handle, if one was spawned.
    pub health_check_handle: Option<JoinHandle<()>>,
    /// Background hot-plug detection loop handle, if one was spawned.
    pub hot_plug_handle: Option<JoinHandle<()>>,
    /// Background OS device bridge loop handle, if one was spawned.
    pub os_bridge_handle: Option<JoinHandle<()>>,
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

        // Register capabilities as tools in ToolRegistry so the LLM can call them.
        // Use register_dynamic since we only have &ToolRegistry (Arc-friendly).
        for cap in &device.capabilities {
            let wrapper = DeviceToolWrapper::new(driver_name.clone(), cap.clone());
            tool_registry.register_dynamic(Arc::new(wrapper));
        }
    }

    let registry = Arc::new(device_registry);

    // Spawn background health-check loop
    let health_check_handle = if config.health_check.interval_secs > 0 {
        let reg = registry.clone();
        let cfg = config.health_check.clone();
        Some(spawn_health_check_loop(reg, cfg))
    } else {
        None
    };

    // Spawn background hot-plug detection loop
    let hot_plug_handle = if config.hot_plug.scan_interval_secs > 0 {
        let reg = registry.clone();
        let cfg = config.hot_plug.clone();
        Some(spawn_hot_plug_loop(reg, cfg))
    } else {
        None
    };

    Ok(Some(DeviceInit {
        registry,
        health_check_handle,
        hot_plug_handle,
        os_bridge_handle: None,
    }))
}

// ── Driver factory (config-driven discovery) ─────────────────────────────

/// Constructor signature for building a device driver from JSON parameters.
type DriverConstructor = fn(Value) -> crate::Result<Arc<dyn DeviceDriver>>;

/// Registry of driver constructors keyed by their config `kind` string.
///
/// Follows the same pattern as the Provider Resolver in
/// [`crate::providers::resolver`] — a centralised mapping from a config-level
/// type name to the concrete constructor function.
///
/// # Example
///
/// ```ignore
/// let factory = DriverFactory::new();
/// let driver = factory.build("mock", json!({ "name": "sensor" }))?;
/// ```
pub struct DriverFactory {
    constructors: HashMap<&'static str, DriverConstructor>,
}

impl DriverFactory {
    /// Create a factory with all built-in driver constructors registered.
    pub fn new() -> Self {
        let mut f = Self {
            constructors: HashMap::new(),
        };
        f.register("mock", MockDeviceDriver::from_config);
        f
    }

    /// Register a driver constructor for the given `kind` string.
    pub fn register(&mut self, kind: &'static str, ctor: DriverConstructor) {
        self.constructors.insert(kind, ctor);
    }

    /// Build a driver by `kind`, passing `params` to its constructor.
    ///
    /// Returns an error if `kind` is not registered.
    pub fn build(&self, kind: &str, params: Value) -> crate::Result<Arc<dyn DeviceDriver>> {
        let ctor = self.constructors.get(kind).ok_or_else(|| {
            SyscityError::NotFound {
                resource: format!("Device driver kind '{}'", kind),
            }
        })?;
        ctor(params)
    }
}

impl Default for DriverFactory {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert [`DeviceDriverEntry`] items from config into driver instances.
///
/// Each entry is built via [`DriverFactory`].  Failures are logged as warnings
/// and skipped so that a single misconfigured entry does not block the entire
/// device subsystem.
pub fn discover_drivers_from_config(config: &DeviceConfig) -> Vec<Arc<dyn DeviceDriver>> {
    let factory = DriverFactory::new();
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
/// Call this after `init_devices()` succeeds. Returns `Some(JoinHandle)` if
/// the bridge was spawned, `None` if disabled or no matchers configured.
pub fn spawn_os_bridge_from_config(
    registry: Arc<DeviceRegistry>,
    os_bridge: &OsBridgeConfig,
    tool_registry: Arc<ToolRegistry>,
) -> Option<JoinHandle<()>> {
    if !os_bridge.enabled || os_bridge.matchers.is_empty() {
        return None;
    }

    let matchers = os_bridge.matchers.clone();
    let build_driver: DriverBuilder = Arc::new(|kind: &str, params: serde_json::Value| {
        DriverFactory::new().build(kind, params)
    });

    Some(spawn_os_bridge_loop(
        registry,
        matchers,
        tool_registry,
        build_driver,
    ))
}

/// Reload the device subsystem from configuration.
///
/// 1. Disconnects all devices in the old registry.
/// 2. Aborts the old health-check loop.
/// 3. Deregisters all old device tools from the [`ToolRegistry`].
/// 4. Re-runs driver discovery and init with the new `config`.
/// 5. Returns the new [`DeviceInit`].
///
/// Returns `Ok(None)` if the device subsystem is disabled or has no drivers.
pub async fn reload_devices(
    old: DeviceInit,
    config: &DeviceConfig,
    tool_registry: &ToolRegistry,
) -> crate::Result<Option<DeviceInit>> {
    // 1. Disconnect all old devices
    old.registry.disconnect_all().await;

    // 2. Abort old loops
    if let Some(handle) = old.health_check_handle {
        handle.abort();
    }
    if let Some(handle) = old.hot_plug_handle {
        handle.abort();
    }
    if let Some(handle) = old.os_bridge_handle {
        handle.abort();
    }

    // 3. Deregister all old device tools
    tool_registry.deregister_prefix("device_");

    // 4. Re-run discovery and init
    let drivers = discover_drivers_from_config(config);
    init_devices(config, drivers, tool_registry).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Capability;
    use crate::device::mock::MockDeviceDriver;
    use crate::gateway::DeviceDriverEntry;
    use async_trait::async_trait;
    use serde_json::json;

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
        let result = init_devices(&config, vec![], &ToolRegistry::new())
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

        let result = init_devices(
            &config,
            vec![Arc::new(driver)],
            &tool_registry,
        )
        .await
        .expect("init_devices should succeed")
        .expect("init_devices should return Some when enabled");

        // Verify the device is registered
        assert_eq!(result.registry.len().await, 1);
        assert!(result.health_check_handle.is_some());

        // Verify tool is registered in ToolRegistry
        let names = tool_registry.list();
        assert!(
            names.contains(&"device_sensor-01_sensor_read_temperature".to_string()),
            "tool should be registered in ToolRegistry: {:?}",
            names,
        );
    }

    #[tokio::test]
    async fn test_init_devices_empty_drivers() {
        let config = DeviceConfig::default();
        let result = init_devices(&config, vec![], &ToolRegistry::new())
            .await
            .expect("init_devices should succeed with no drivers");
        assert!(result.is_none());
    }

    // ── DriverFactory tests ────────────────────────────────────────────────

    #[test]
    fn test_driver_factory_build_mock() {
        let factory = DriverFactory::new();
        let driver = factory
            .build("mock", json!({ "name": "cfg-motor", "present": true }))
            .expect("mock driver should build");
        assert_eq!(driver.driver_name(), "cfg-motor");
    }

    #[test]
    fn test_driver_factory_build_unknown() {
        let factory = DriverFactory::new();
        let result = factory.build("nonexistent", json!({}));
        match result {
            Ok(_) => panic!("expected error for unknown driver kind"),
            Err(e) => assert!(e.to_string().contains("nonexistent")),
        }
    }

    #[test]
    fn test_driver_factory_build_mock_defaults() {
        let factory = DriverFactory::new();
        let driver = factory
            .build("mock", json!({}))
            .expect("mock driver with empty params should build");
        assert_eq!(driver.driver_name(), "mock"); // default name
    }

    #[test]
    fn test_discover_drivers_from_config_empty() {
        let config = DeviceConfig::default(); // no drivers
        let drivers = discover_drivers_from_config(&config);
        assert!(drivers.is_empty());
    }

    #[test]
    fn test_discover_drivers_from_config_with_mock() {
        let config = DeviceConfig {
            drivers: vec![DeviceDriverEntry {
                kind: "mock".into(),
                params: json!({ "name": "sensor-01", "present": true }),
            }],
            ..Default::default()
        };
        let drivers = discover_drivers_from_config(&config);
        assert_eq!(drivers.len(), 1);
        assert_eq!(drivers[0].driver_name(), "sensor-01");
    }

    #[test]
    fn test_discover_drivers_from_config_skips_unknown() {
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
        let drivers = discover_drivers_from_config(&config);
        assert_eq!(drivers.len(), 1);
        assert_eq!(drivers[0].driver_name(), "good");
    }
}
