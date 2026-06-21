//! End-to-end device integration tests.
//!
//! Verifies that mock device drivers can be injected into a test gateway,
//! their capabilities are registered as tools in ToolRegistry, and the
//! LLM can discover and call device operations through standard function
//! calling — no Agent changes needed.

use std::sync::Arc;

use syscity::device::mock::{MockCapability, MockDeviceDriver};
use syscity::device::Capability;
use syscity::device::DeviceDriver;
use syscity::model_router::ModelAlias;

use super::*;

// Re-exported from super: MockProvider, ProviderMessage, Role, ToolCall,
// FunctionCall, Gateway, test_config, json, timeout, Duration,
// FrontendSimulator, Message

#[tokio::test]
#[serial_test::serial]
async fn device_tool_registered_in_gateway() {
    let port = 18100;

    let sensor_cap = Arc::new(
        MockCapability::new("sensor.read_temperature")
            .with_result(serde_json::json!({"celsius": 23.5})),
    );
    let driver = MockDeviceDriver::new("sensor-01", true)
        .with_capabilities(vec![sensor_cap as Arc<dyn Capability>]);

    let config = test_config(port, false);
    let gateway =
        Gateway::with_devices(config, None, vec![Arc::new(driver) as Arc<dyn DeviceDriver>])
            .await
            .expect("Failed to create gateway with devices");

    // Verify the device tool is registered in ToolRegistry
    let registry = gateway.tool_registry();
    let tool_names = registry.list();
    assert!(
        tool_names.contains(&"device_sensor-01_sensor_read_temperature".to_string()),
        "Device tool should be registered: {:?}",
        tool_names,
    );

    // Verify the device registry is present and has the connected device
    let device_registry = gateway.device_registry();
    assert!(device_registry.is_some(), "Device registry should be present");
    let device_reg = device_registry.unwrap();
    assert_eq!(device_reg.len().await, 1);
}

#[tokio::test]
#[serial_test::serial]
async fn device_tool_invoked_via_chat() {
    let port = 18101;

    // ── Build mock device driver with a temperature sensor capability ──
    let sensor_cap: Arc<dyn Capability> = Arc::new(
        MockCapability::new("sensor.read_temperature")
            .with_result(serde_json::json!({"celsius": 23.5})),
    );
    let driver: Arc<dyn DeviceDriver> =
        Arc::new(MockDeviceDriver::new("sensor-01", true).with_capabilities(vec![sensor_cap]));

    // ── Build mock provider that will call the device tool ──
    let mock = MockProvider::new().with_callback(move |messages| {
        // Handle NOCACHE handshake
        if messages.len() == 1 && messages[0].content.contains("NOCACHE") {
            return ProviderMessage::assistant("NOCACHE");
        }

        let has_tool_result = messages.iter().any(|m| m.role == Role::Tool);
        if has_tool_result {
            return ProviderMessage::assistant("Done! Sensor reading obtained.");
        }

        // First turn: emit a tool call for the device sensor tool
        ProviderMessage::assistant("I'll read the sensor.").with_tool_calls(vec![ToolCall {
            id: "call_device_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "device_sensor-01_sensor_read_temperature".to_string(),
                arguments: "{}".to_string(),
            },
            index: None,
            result: None,
        }])
    });

    // ── Start gateway with both mock device and mock provider ──
    let mut config = test_config(port, false);
    config.model_provider = "mock".to_string();
    config.model = "mock-model".to_string();

    let gateway = Gateway::with_devices(config, None, vec![driver])
        .await
        .expect("Failed to create gateway");

    let router = gateway.model_router();
    router
        .add_provider_instance("mock", Arc::new(mock))
        .await
        .expect("Failed to register mock provider");
    router
        .set_alias(ModelAlias {
            name: "mock-model".to_string(),
            provider: "mock".to_string(),
            model: "mock-model".to_string(),
            temperature: None,
            max_tokens: None,
        })
        .await;

    tokio::spawn(async move {
        let _ = gateway.start().await;
    });

    // ── Wait for gateway to be ready ──
    let url = format!("ws://127.0.0.1:{}/ws", port);
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if tokio_tungstenite::connect_async(&url).await.is_ok() {
            break;
        }
    }

    // ── Connect and send a chat message ──
    let mut client = FrontendSimulator::connect(port).await;
    let sid = client.create_session().await;
    client.subscribe(vec![sid.clone()]).await;
    client.send_chat(&sid, "Read the temperature sensor").await;

    // ── Collect events and verify the device tool was called ──
    let result = timeout(Duration::from_secs(30), async {
        let mut tool_called = false;
        let mut tool_result_received = false;
        let mut tool_result_output = String::new();

        while let Some(msg) = client.read.next().await {
            let msg = msg.unwrap();
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                    if event.get("type").and_then(|v| v.as_str()) == Some("event") {
                        let name = event.get("event").and_then(|v| v.as_str());
                        let payload = event.get("payload").cloned();
                        match name {
                            Some("tool.calling") => {
                                if let Some(ref p) = payload {
                                    if p.get("tool_name").and_then(|v| v.as_str())
                                        == Some("device_sensor-01_sensor_read_temperature")
                                    {
                                        tool_called = true;
                                    }
                                }
                            }
                            Some("tool.result") => {
                                if let Some(p) = payload {
                                    if p.get("tool_name").and_then(|v| v.as_str())
                                        == Some("device_sensor-01_sensor_read_temperature")
                                    {
                                        tool_result_received = true;
                                        if let Some(r) = p.get("result").and_then(|v| v.as_str()) {
                                            tool_result_output = r.to_string();
                                        }
                                    }
                                }
                            }
                            Some("chat.final") => {
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        (tool_called, tool_result_received, tool_result_output)
    })
    .await;

    let (tool_called, tool_result_received, tool_result_output) =
        result.expect("Timed out waiting for chat completion");

    assert!(tool_called, "Device tool should have been called");
    assert!(tool_result_received, "Device tool result should have been received");
    assert!(
        tool_result_output.contains("23.5"),
        "Tool result should contain sensor reading: {}",
        tool_result_output,
    );
}

#[tokio::test]
#[serial_test::serial]
async fn test_device_registry_accessible_from_gateway() {
    let port = 18200;

    let sensor_cap = Arc::new(
        MockCapability::new("sensor.read_temperature")
            .with_result(serde_json::json!({"celsius": 23.5})),
    );
    let driver = MockDeviceDriver::new("sensor-01", true)
        .with_capabilities(vec![sensor_cap as Arc<dyn Capability>]);

    let config = test_config(port, false);
    let gateway =
        Gateway::with_devices(config, None, vec![Arc::new(driver) as Arc<dyn DeviceDriver>])
            .await
            .expect("Failed to create gateway with devices");

    let device_registry = gateway.device_registry();
    assert!(device_registry.is_some(), "Device registry should be present");
    let reg = device_registry.unwrap();

    assert_eq!(reg.len().await, 1, "Should have 1 connected device");
    let ids = reg.list().await;
    assert!(ids.contains(&"dev-sensor-01".to_string()), "Device ID should be connected");
}

#[tokio::test]
#[serial_test::serial]
async fn test_status_events_available_through_gateway_registry() {
    let port = 18201;

    let sensor_cap = Arc::new(
        MockCapability::new("sensor.read_temperature")
            .with_result(serde_json::json!({"celsius": 23.5})),
    );
    let driver = MockDeviceDriver::new("sensor-01", true)
        .with_capabilities(vec![sensor_cap as Arc<dyn Capability>]);

    let config = test_config(port, false);
    let gateway =
        Gateway::with_devices(config, None, vec![Arc::new(driver) as Arc<dyn DeviceDriver>])
            .await
            .expect("Failed to create gateway with devices");

    let device_registry = gateway
        .device_registry()
        .expect("Device registry should be present");

    // Subscribe to status events
    let mut rx = device_registry.subscribe_status();

    // Perform a disconnect to trigger a status event
    device_registry.disconnect("dev-sensor-01").await.unwrap();

    // Verify we received the Disconnected event
    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("should receive event within timeout")
        .expect("event should not be lagged");

    assert_eq!(event.device_id, "dev-sensor-01");
    // Note: event.current reflects the status read from the device after the
    // operation
}
