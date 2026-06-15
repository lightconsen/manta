//! Integration tests for device infrastructure using mock drivers.
//!
//! These tests validate the full lifecycle: create mock capabilities and
//! drivers, build a device registry, probe, connect, and exercise mock
//! capabilities directly.

use syscity::device::Capability;
use syscity::device::mock::{make_mock_device_registry, MockCapability, MockDeviceDriver};
use std::sync::Arc;

/// Full flow: mock driver -> probe -> connect -> verify device state.
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
    assert_eq!(device.capabilities.len(), 2);
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

    assert_eq!(dev1.capabilities.len(), 2);
    assert_eq!(dev2.capabilities.len(), 1);
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
