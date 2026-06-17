//! End-to-end perception integration tests.
//!
//! Verifies that the perception fusion layer is wired into the gateway:
//! - Perception registry is created when perception is enabled
//! - Device capabilities are registered as perception sources
//! - perception_query tool is registered in the tool registry
//! - Poll-and-query works via the perception registry API
//!
//! Each gateway-bearing test uses a different storage type to avoid
//! SQLite global-state conflicts when multiple gateways are created
//! in the same process.

use std::sync::Arc;

use syscity::device::mock::{MockCapability, MockDeviceDriver};
use syscity::device::{Capability, DeviceDriver};

use super::*;

/// Create a config with perception enabled and in-memory storage to
/// avoid SQLite file-lock conflicts between sequential tests.
fn perception_config(port: u16) -> GatewayConfig {
    let mut config = test_config(port, false);
    config.storage.storage_type = "memory".to_string();
    config.perception.enabled = true;
    config
}

#[tokio::test]
#[serial_test::serial]
async fn perception_registry_has_device_sources() {
    let port = 18500;

    let temp_cap: Arc<dyn Capability> = Arc::new(
        MockCapability::new("sensor.read_temperature")
            .with_result(serde_json::json!({"celsius": 23.5})),
    );
    let driver: Arc<dyn DeviceDriver> = Arc::new(
        MockDeviceDriver::new("sensor-01", true)
            .with_capabilities(vec![temp_cap]),
    );

    let config = perception_config(port);
    let gateway = Gateway::with_devices(config, None, vec![driver])
        .await
        .expect("Failed to create gateway with devices");

    // Perception registry is present with device sources
    let reg = gateway.perception_registry().expect("Perception registry should be present");
    let sources = reg.list_sources().await;
    assert!(!sources.is_empty(), "Should have registered sources");
    assert!(
        sources.contains(&"device:sensor-01:sensor.read_temperature".to_string()),
        "Device capabilities should be registered as perception sources: {:?}",
        sources,
    );

    // perception_query tool is registered
    let tool_names = gateway.tool_registry().list();
    assert!(
        tool_names.contains(&"perception_query".to_string()),
        "perception_query tool should be registered: {:?}",
        tool_names,
    );
}

#[tokio::test]
#[serial_test::serial]
async fn perception_poll_and_query_works() {
    // Uses PerceptionRegistry directly (not via Gateway) to verify the
    // poll-and-query flow, avoiding any gateway/storage dependencies.
    let reg = Arc::new(syscity::perception::PerceptionRegistry::new(
        syscity::perception::AggregationStrategy::Latest,
        10,
    ));

    reg.register_source(Arc::new(
        syscity::perception::mock::MockPerceptionSource::new("e2e_test_sensor")
            .with_modality(syscity::perception::Modality::Device)
            .with_data(serde_json::json!({"value": 42})),
    ))
    .await;

    reg.poll_all().await;
    let result = reg
        .query(&syscity::perception::PerceptionQuery::default())
        .await;

    assert!(!result.entities.is_empty(), "Expected entities after poll_all");
    let entity_ids: Vec<String> = result.entities.iter().map(|e| e.id.to_string()).collect();
    assert!(
        entity_ids.contains(&"e2e_test_sensor".to_string()),
        "Expected entity for e2e_test_sensor, got: {:?}",
        entity_ids,
    );
}

#[tokio::test]
#[serial_test::serial]
async fn perception_with_devices_and_tool_registered() {
    let port = 18501;

    let temp_cap: Arc<dyn Capability> = Arc::new(
        MockCapability::new("sensor.read_temperature")
            .with_result(serde_json::json!({"celsius": 23.5})),
    );
    let driver: Arc<dyn DeviceDriver> = Arc::new(
        MockDeviceDriver::new("sensor-01", true)
            .with_capabilities(vec![temp_cap]),
    );

    let config = perception_config(port);
    let gateway = Gateway::with_devices(config, None, vec![driver])
        .await
        .expect("Failed to create gateway with devices");

    // Device capabilities are registered as perception sources
    let perception_registry = gateway.perception_registry().expect("registry present");
    let sources = perception_registry.list_sources().await;
    assert!(
        sources.contains(&"device:sensor-01:sensor.read_temperature".to_string()),
        "Device capabilities should be perception sources: {:?}",
        sources,
    );

    // Both device tool and perception_query tool are registered
    let tool_names = gateway.tool_registry().list();
    assert!(
        tool_names.contains(&"perception_query".to_string()),
        "perception_query tool should be registered"
    );
    assert!(
        tool_names.contains(&"device_sensor-01_sensor_read_temperature".to_string()),
        "Device tool should also be registered"
    );
}
