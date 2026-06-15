//! Device subsystem initialization.
//!
//! Discovers, probes, and connects physical devices.  Each device capability
//! is wrapped as a [`DeviceToolWrapper`] and registered in [`ToolRegistry`] so
//! the LLM can discover and call device operations through standard tool
//! function calling — no Agent changes needed.

use std::sync::Arc;

use crate::device::registry::DeviceRegistry;
use crate::device::DeviceDriver;
use crate::tools::device_tool::DeviceToolWrapper;
use crate::tools::ToolRegistry;

/// Device subsystem initialization result.
pub struct DeviceInit {
    /// Registry managing all discovered devices and their lifecycle
    /// (health checks, reconnect, disconnect).
    pub registry: Arc<DeviceRegistry>,
}

/// Initialize the device subsystem.
///
/// 1. Registers each driver in a fresh [`DeviceRegistry`].
/// 2. Probes all drivers to discover present hardware.
/// 3. Connects each present device.
/// 4. Wraps each capability as a [`DeviceToolWrapper`] and registers it in
///    `tool_registry` so the LLM can discover and call device operations.
///
/// Returns a [`DeviceInit`] that the caller should store for lifecycle
/// management (health checks, reconnect on failure, shutdown).
pub async fn init_devices(
    drivers: Vec<Arc<dyn DeviceDriver>>,
    tool_registry: &ToolRegistry,
) -> crate::Result<DeviceInit> {
    let mut device_registry = DeviceRegistry::new();

    for driver in drivers {
        device_registry.register(driver);
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

    Ok(DeviceInit {
        registry: Arc::new(device_registry),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Capability;
    use crate::device::mock::MockDeviceDriver;
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
    async fn test_init_devices_with_mock_drivers() {
        let sensor = Arc::new(DummySensor);
        let driver = MockDeviceDriver::new("sensor-01", true)
            .with_capabilities(vec![sensor as Arc<dyn Capability>]);

        let tool_registry = ToolRegistry::new();

        let result = init_devices(
            vec![Arc::new(driver)],
            &tool_registry,
        )
        .await
        .expect("init_devices should succeed");

        // Verify the device is registered
        assert_eq!(result.registry.len().await, 1);

        // Verify tool is registered in ToolRegistry (dynamic tools show in list)
        let names = tool_registry.list();
        assert!(
            names.contains(&"device_sensor-01_sensor_read_temperature".to_string()),
            "tool should be registered in ToolRegistry: {:?}",
            names,
        );
    }
}
