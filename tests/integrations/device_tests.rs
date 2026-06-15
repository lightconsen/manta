//! Integration tests for device infrastructure using mock drivers.
//!
//! These tests validate the full lifecycle: create mock capabilities and
//! drivers → device registry → register into CapabilityRegistry →
//! resolve by name.

use syscity::capability::registry::CapabilityRegistry;
use syscity::capability::providers::device_adapter;
use syscity::capability::Capability;
use syscity::device::mock::{make_mock_device_registry, MockCapability, MockDeviceDriver};
use std::sync::Arc;

/// Full flow: mock driver → probe → connect → register capabilities →
/// verify in CapabilityRegistry.
#[tokio::test]
async fn mock_device_full_lifecycle() {
    let caps: Vec<Arc<dyn Capability>> = vec![
        Arc::new(MockCapability::new("motor.move_to").with_schema(serde_json::json!({
            "type": "object",
            "properties": {
                "position": { "type": "integer" }
            }
        }))),
        Arc::new(MockCapability::new("motor.stop")),
    ];

    let driver = MockDeviceDriver::new("stepper", true)
        .with_capabilities(caps);

    // Build registry and connect
    let reg = make_mock_device_registry(vec![driver]);
    let available = reg.probe_all().await.unwrap();
    assert_eq!(available, vec!["stepper"]);

    let device = reg.connect("stepper").await.unwrap();
    assert_eq!(device.id(), "dev-stepper");
    assert!(device.status.read().await.is_connected());

    // Register device capabilities
    let mut cap_reg = CapabilityRegistry::new();
    device_adapter::register_device_capabilities(&mut cap_reg, device);

    // Verify capabilities are discoverable
    assert!(cap_reg.resolve("motor.move_to").is_some());
    assert!(cap_reg.resolve("motor.stop").is_some());
    assert_eq!(cap_reg.len(), 2);

    // List by prefix
    let motor_caps = cap_reg.list_by_prefix("motor");
    assert_eq!(motor_caps.len(), 2);
    assert!(motor_caps.contains(&"motor.move_to".to_string()));
    assert!(motor_caps.contains(&"motor.stop".to_string()));
}

/// Execute a mock capability and verify the result.
#[tokio::test]
async fn mock_device_execute_capability() {
    let cap = MockCapability::new("sensor.read_temp")
        .with_result(serde_json::json!({"celsius": 23.5}));

    let result = cap.execute(serde_json::json!({})).await;
    assert!(result.success);
    assert_eq!(
        result.output,
        Some(serde_json::json!({"celsius": 23.5}))
    );
}

/// Error path: driver that fails to connect.
#[tokio::test]
async fn mock_device_connect_error() {
    let driver = MockDeviceDriver::new("faulty", true)
        .with_connect_error("hardware not responding");

    let reg = make_mock_device_registry(vec![driver]);
    let available = reg.probe_all().await.unwrap();
    assert_eq!(available, vec!["faulty"]);

    let err = reg.connect("faulty").await.unwrap_err();
    assert!(err.to_string().contains("hardware not responding"));
}

/// Multiple mock devices with different capabilities.
#[tokio::test]
async fn mock_device_multiple_devices() {
    let motor_caps: Vec<Arc<dyn Capability>> = vec![
        Arc::new(MockCapability::new("motor.move_to")),
        Arc::new(MockCapability::new("motor.stop")),
    ];
    let camera_caps: Vec<Arc<dyn Capability>> = vec![
        Arc::new(MockCapability::new("camera.capture")),
    ];

    let drivers = vec![
        MockDeviceDriver::new("stepper", true)
            .with_capabilities(motor_caps),
        MockDeviceDriver::new("webcam", true)
            .with_capabilities(camera_caps),
    ];

    let reg = make_mock_device_registry(drivers);
    let available = reg.probe_all().await.unwrap();
    assert_eq!(available.len(), 2);

    let dev1 = reg.connect("stepper").await.unwrap();
    let dev2 = reg.connect("webcam").await.unwrap();

    let mut cap_reg = CapabilityRegistry::new();
    device_adapter::register_device_capabilities(&mut cap_reg, dev1);
    device_adapter::register_device_capabilities(&mut cap_reg, dev2);

    assert_eq!(cap_reg.len(), 3);
    assert!(cap_reg.resolve("motor.move_to").is_some());
    assert!(cap_reg.resolve("motor.stop").is_some());
    assert!(cap_reg.resolve("camera.capture").is_some());
}

/// Device registry lifecycle: disconnect and health check.
#[tokio::test]
async fn mock_device_lifecycle_management() {
    let driver = MockDeviceDriver::new("sensor", true)
        .with_capabilities(vec![Arc::new(MockCapability::new("sensor.read"))]);

    let reg = make_mock_device_registry(vec![driver]);
    reg.connect("sensor").await.unwrap();
    assert_eq!(reg.len().await, 1);

    // Health check (mock returns Ok(true) by default)
    assert!(reg.health_check("dev-sensor").await.unwrap());

    // Disconnect
    reg.disconnect("dev-sensor").await.unwrap();
    assert!(reg.is_empty().await);
}
