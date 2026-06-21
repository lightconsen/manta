//! Integration tests for device infrastructure using mock drivers.
//!
//! These tests validate the full lifecycle: create mock capabilities and
//! drivers, build a device registry, probe, connect, and exercise mock
//! capabilities directly.

use std::sync::Arc;

use syscity::device::mock::{make_mock_device_registry, MockCapability, MockDeviceDriver};
use syscity::device::Capability;
use syscity::device::{DeviceStatus, HealthCheckConfig, HotPlugConfig};
use syscity::gateway::init::devices::init_devices;
use syscity::gateway::DeviceConfig;
use syscity::tools::ToolRegistry;

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

    let driver = MockDeviceDriver::new("stepper", true).with_capabilities(caps);

    // Build registry and connect
    let reg = make_mock_device_registry(vec![driver]).await;
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
    let cap =
        MockCapability::new("sensor.read_temp").with_result(serde_json::json!({"celsius": 23.5}));

    let result = cap.execute(serde_json::json!({})).await;
    assert!(result.success);
    assert_eq!(result.output, Some(serde_json::json!({"celsius": 23.5})));
}

/// Error path: driver that fails to connect.
#[tokio::test]
async fn mock_device_connect_error() {
    let driver =
        MockDeviceDriver::new("faulty", true).with_connect_error("hardware not responding");

    let reg = make_mock_device_registry(vec![driver]).await;
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
    let camera_caps: Vec<Arc<dyn Capability>> =
        vec![Arc::new(MockCapability::new("camera.capture"))];

    let drivers = vec![
        MockDeviceDriver::new("stepper", true).with_capabilities(motor_caps),
        MockDeviceDriver::new("webcam", true).with_capabilities(camera_caps),
    ];

    let reg = make_mock_device_registry(drivers).await;
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

    let reg = make_mock_device_registry(vec![driver]).await;
    reg.connect("sensor").await.unwrap();
    assert_eq!(reg.len().await, 1);

    // Health check (mock returns Ok(true) by default)
    assert!(reg.health_check("dev-sensor").await.unwrap());

    // Disconnect
    reg.disconnect("dev-sensor").await.unwrap();
    assert!(reg.is_empty().await);
}

/// Full lifecycle via init_devices: connect, subscribe, disconnect, verify
/// status events on the broadcast bus.
#[tokio::test]
async fn test_status_bus_full_lifecycle() {
    let sensor = Arc::new(MockCapability::new("sensor.read"));
    let driver = MockDeviceDriver::new("sensor-01", true)
        .with_capabilities(vec![sensor as Arc<dyn Capability>]);
    let config = DeviceConfig::default();
    let tool_registry = ToolRegistry::new();

    let init = init_devices(&config, vec![Arc::new(driver)], &tool_registry, None)
        .await
        .expect("init should succeed")
        .expect("init should return Some");

    let mut rx = init.registry.subscribe_status();

    // Disconnect to trigger event
    init.registry.disconnect("dev-sensor-01").await.unwrap();

    let event = rx.try_recv().expect("expected status event");
    assert_eq!(event.device_id, "dev-sensor-01");
    assert!(matches!(event.current, DeviceStatus::Disconnected));

    // Clean up background handles
    if let Some(h) = init.health_check_handle {
        h.abort();
    }
    if let Some(h) = init.hot_plug_handle {
        h.abort();
    }
}

/// Verify that reconnect emits both Disconnected and Connected status events.
#[tokio::test]
async fn test_status_bus_reconnect_events() {
    let sensor = Arc::new(MockCapability::new("sensor.read"));
    let driver = MockDeviceDriver::new("sensor-01", true)
        .with_capabilities(vec![sensor as Arc<dyn Capability>]);
    let config = DeviceConfig::default();
    let tool_registry = ToolRegistry::new();

    let init = init_devices(&config, vec![Arc::new(driver)], &tool_registry, None)
        .await
        .expect("init should succeed")
        .expect("init should return Some");

    let mut rx = init.registry.subscribe_status();
    init.registry.reconnect("dev-sensor-01").await.unwrap();

    // Drain events: should have Disconnected then Connected
    let mut events = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(e) => events.push(e.current),
            Err(_) => break,
        }
    }

    assert!(events
        .iter()
        .any(|s| matches!(s, DeviceStatus::Disconnected)));
    assert!(events
        .iter()
        .any(|s| matches!(s, DeviceStatus::Connected { .. })));

    if let Some(h) = init.health_check_handle {
        h.abort();
    }
    if let Some(h) = init.hot_plug_handle {
        h.abort();
    }
}

/// Verify that init_devices spawns a health-check loop when the interval is >
/// 0.
#[tokio::test]
async fn test_init_devices_spawns_health_loop() {
    let sensor = Arc::new(MockCapability::new("sensor.read"));
    let driver = MockDeviceDriver::new("sensor-01", true)
        .with_capabilities(vec![sensor as Arc<dyn Capability>]);
    let config = DeviceConfig {
        health_check: HealthCheckConfig {
            interval_secs: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let tool_registry = ToolRegistry::new();
    let init = init_devices(&config, vec![Arc::new(driver)], &tool_registry, None)
        .await
        .expect("init should succeed")
        .expect("init should return Some");

    assert!(init.health_check_handle.is_some());
    assert!(init.hot_plug_handle.is_some());

    if let Some(h) = init.health_check_handle {
        h.abort();
    }
    if let Some(h) = init.hot_plug_handle {
        h.abort();
    }
}

/// Verify that init_devices does not spawn background loops when intervals are
/// 0.
#[tokio::test]
async fn test_init_devices_no_handle_when_interval_zero() {
    let sensor = Arc::new(MockCapability::new("sensor.read"));
    let driver = MockDeviceDriver::new("sensor-01", true)
        .with_capabilities(vec![sensor as Arc<dyn Capability>]);
    let config = DeviceConfig {
        health_check: HealthCheckConfig {
            interval_secs: 0,
            ..Default::default()
        },
        hot_plug: HotPlugConfig {
            scan_interval_secs: 0,
            ..Default::default()
        },
        ..Default::default()
    };
    let tool_registry = ToolRegistry::new();
    let init = init_devices(&config, vec![Arc::new(driver)], &tool_registry, None)
        .await
        .expect("init should succeed")
        .expect("init should return Some");

    assert!(init.health_check_handle.is_none());
    assert!(init.hot_plug_handle.is_none());
}

/// Use make_mock_device_registry with multiple drivers that differ in health,
/// then verify health_check_all returns the correct per-device status.
#[tokio::test]
async fn test_multiple_devices_health_check_all() {
    let good = MockDeviceDriver::new("good", true).with_health(true);
    let bad = MockDeviceDriver::new("bad", true).with_health(false);

    let reg = make_mock_device_registry(vec![good, bad]).await;
    reg.connect("good").await.unwrap();
    reg.connect("bad").await.unwrap();

    let results = reg.health_check_all().await;
    assert_eq!(results.len(), 2);
    assert!(results.get("dev-good").copied().unwrap_or(false));
    assert!(!results.get("dev-bad").copied().unwrap_or(true));
}
