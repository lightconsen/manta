//! Mock implementations for testing device infrastructure.
//!
//! Provides [`MockCapability`] and [`MockDeviceDriver`] for use in unit,
//! integration, and E2E tests — mirroring the pattern established by
//! [`crate::providers::mock::MockProvider`].
//!
//! # Examples
//!
//! ```ignore
//! use syscity::device::mock::{MockCapability, MockDeviceDriver};
//! use std::sync::Arc;
//!
//! let cap = MockCapability::new("motor.move_to")
//!     .with_result(serde_json::json!({"position": 42}));
//!
//! let driver = MockDeviceDriver::new("stepper-motor", true)
//!     .with_capabilities(vec![Arc::new(cap)]);
//! ```

use crate::device::safety::{SafetyRule, SafetyRuleKind, SafetyZone};
use crate::device::capability::{Capability, CapabilityResult};
use crate::device::driver::DeviceDriver;
use crate::device::registry::DeviceRegistry;
use crate::device::{Device, DeviceInfo};
use crate::error::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

// ── MockCapability ────────────────────────────────────────────────────────────

/// A configurable [`Capability`] implementation for testing.
///
/// By default returns `CapabilityResult { success: true, output: None,
/// error: None, duration_ms: 0 }` and an empty `param_schema`.
pub struct MockCapability {
    name: String,
    param_schema: Value,
    execute_fn: Box<dyn Fn(Value) -> CapabilityResult + Send + Sync>,
}

impl MockCapability {
    /// Create a mock capability with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            name: n,
            param_schema: Value::Null,
            execute_fn: Box::new(|_| CapabilityResult {
                success: true,
                output: None,
                error: None,
                duration_ms: 0,
            }),
        }
    }

    /// Set the JSON Schema returned by `param_schema`.
    pub fn with_schema(mut self, schema: Value) -> Self {
        self.param_schema = schema;
        self
    }

    /// Override the execute behaviour with a custom function.
    pub fn with_execute(
        mut self,
        f: Box<dyn Fn(Value) -> CapabilityResult + Send + Sync>,
    ) -> Self {
        self.execute_fn = f;
        self
    }

    /// Make execution return a successful result with the given output.
    pub fn with_result(mut self, output: Value) -> Self {
        let output_clone = output;
        self.execute_fn = Box::new(move |_| CapabilityResult {
            success: true,
            output: Some(output_clone.clone()),
            error: None,
            duration_ms: 0,
        });
        self
    }

    /// Make execution fail with the given error message.
    pub fn with_error(mut self, msg: impl Into<String>) -> Self {
        let m = msg.into();
        self.execute_fn = Box::new(move |_| CapabilityResult {
            success: false,
            output: None,
            error: Some(m.clone()),
            duration_ms: 0,
        });
        self
    }

    /// Set the duration reported in the result (default 0).
    pub fn with_duration(mut self, ms: u64) -> Self {
        let old_fn = std::mem::replace(
            &mut self.execute_fn,
            Box::new(|_| unreachable!()),
        );
        self.execute_fn = Box::new(move |params| {
            let mut result = old_fn(params);
            result.duration_ms = ms;
            result
        });
        self
    }
}

#[async_trait]
impl Capability for MockCapability {
    fn name(&self) -> &str {
        &self.name
    }

    fn param_schema(&self) -> Value {
        self.param_schema.clone()
    }

    async fn execute(&self, params: Value) -> CapabilityResult {
        (self.execute_fn)(params)
    }
}

// ── MockDeviceDriver ──────────────────────────────────────────────────────────

/// A configurable [`DeviceDriver`] implementation for testing.
///
/// # Default behaviour
///
/// | Method         | Behaviour                              |
/// |----------------|----------------------------------------|
/// | `probe`        | Returns `Ok(probe_result)`             |
/// | `connect`      | Returns `Device::connected(...)` with   |
/// |                | configured capabilities or an error    |
/// | `health_check` | Returns `Ok(health_result)`            |
pub struct MockDeviceDriver {
    name: String,
    probe_result: bool,
    capabilities: Vec<Arc<dyn Capability>>,
    connect_error: Option<String>,
    health_result: bool,
}

impl MockDeviceDriver {
    /// Create a mock driver with the given name and probe result.
    pub fn new(name: impl Into<String>, probe_result: bool) -> Self {
        Self {
            name: name.into(),
            probe_result,
            capabilities: Vec::new(),
            connect_error: None,
            health_result: true,
        }
    }

    /// Set the capabilities the connected device will expose.
    pub fn with_capabilities(mut self, caps: Vec<Arc<dyn Capability>>) -> Self {
        self.capabilities = caps;
        self
    }

    /// Make `connect` return an error instead of a device.
    pub fn with_connect_error(mut self, msg: impl Into<String>) -> Self {
        self.connect_error = Some(msg.into());
        self
    }

    /// Set the return value of `health_check` (default `true`).
    pub fn with_health(mut self, ok: bool) -> Self {
        self.health_result = ok;
        self
    }
}

#[async_trait]
impl DeviceDriver for MockDeviceDriver {
    fn driver_name(&self) -> &str {
        &self.name
    }

    async fn probe(&self) -> Result<bool> {
        Ok(self.probe_result)
    }

    async fn connect(&self) -> Result<Device> {
        if let Some(ref err) = self.connect_error {
            return Err(crate::error::SyscityError::Internal(err.clone()));
        }

        let info = DeviceInfo {
            id: format!("dev-{}", self.name),
            model: self.name.clone(),
            firmware_version: Some("mock-1.0".into()),
            location: Some("test-bench".into()),
        };

        Ok(Device::connected(
            info,
            self.capabilities.clone(),
            SafetyZone::new(vec![SafetyRule {
                name: "mock-default".into(),
                kind: SafetyRuleKind::RequiresApproval,
            }]),
        ))
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(self.health_result)
    }
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Build a [`DeviceRegistry`] populated with the given mock drivers.
///
/// Each driver is registered without probing or connecting; call
/// `probe_all()` / `connect()` on the returned registry as needed.
pub fn make_mock_device_registry(drivers: Vec<MockDeviceDriver>) -> DeviceRegistry {
    let mut reg = DeviceRegistry::new();
    for d in drivers {
        reg.register(Arc::new(d));
    }
    reg
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── MockCapability tests ───────────────────────────────────────────────

    #[test]
    fn test_mock_capability_default() {
        let cap = MockCapability::new("test.op");
        assert_eq!(cap.name(), "test.op");
        assert_eq!(cap.param_schema(), Value::Null);
    }

    #[tokio::test]
    async fn test_mock_capability_default_execute() {
        let cap = MockCapability::new("test.op");
        let result = cap.execute(json!({})).await;
        assert!(result.success);
        assert!(result.output.is_none());
        assert!(result.error.is_none());
        assert_eq!(result.duration_ms, 0);
    }

    #[tokio::test]
    async fn test_mock_capability_with_result() {
        let cap = MockCapability::new("test.op")
            .with_result(json!({"key": "value"}))
            .with_schema(json!({"type": "object"}));
        assert_eq!(cap.param_schema(), json!({"type": "object"}));
        let result = cap.execute(json!({})).await;
        assert!(result.success);
        assert_eq!(result.output, Some(json!({"key": "value"})));
    }

    #[tokio::test]
    async fn test_mock_capability_with_error() {
        let cap = MockCapability::new("test.op").with_error("something broke");
        let result = cap.execute(json!({})).await;
        assert!(!result.success);
        assert_eq!(result.error, Some("something broke".into()));
    }

    #[tokio::test]
    async fn test_mock_capability_with_duration() {
        let cap = MockCapability::new("test.op")
            .with_result(json!("ok"))
            .with_duration(42);
        let result = cap.execute(json!({})).await;
        assert_eq!(result.duration_ms, 42);
    }

    #[tokio::test]
    async fn test_mock_capability_custom_execute() {
        let cap = MockCapability::new("echo").with_execute(Box::new(|params| {
            CapabilityResult {
                success: true,
                output: Some(params),
                error: None,
                duration_ms: 1,
            }
        }));
        let result = cap.execute(json!({"msg": "hi"})).await;
        assert_eq!(result.output, Some(json!({"msg": "hi"})));
    }

    // ── MockDeviceDriver tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_mock_driver_probe_true() {
        let driver = MockDeviceDriver::new("sensor", true);
        assert_eq!(driver.driver_name(), "sensor");
        assert!(driver.probe().await.unwrap());
    }

    #[tokio::test]
    async fn test_mock_driver_probe_false() {
        let driver = MockDeviceDriver::new("sensor", false);
        assert!(!driver.probe().await.unwrap());
    }

    #[tokio::test]
    async fn test_mock_driver_connect() {
        let cap = MockCapability::new("read.temp");
        let driver = MockDeviceDriver::new("thermometer", true)
            .with_capabilities(vec![Arc::new(cap)]);

        let device = driver.connect().await.unwrap();
        assert_eq!(device.id(), "dev-thermometer");
        assert_eq!(device.model(), "thermometer");
        assert_eq!(device.capabilities.len(), 1);
        assert_eq!(device.capabilities[0].name(), "read.temp");
    }

    #[tokio::test]
    async fn test_mock_driver_connect_error() {
        let driver = MockDeviceDriver::new("broken", true)
            .with_connect_error("device not found");
        let err = driver.connect().await.unwrap_err();
        assert!(err.to_string().contains("device not found"));
    }

    #[tokio::test]
    async fn test_mock_driver_health_check() {
        let driver = MockDeviceDriver::new("ok-device", true);
        assert!(driver.health_check().await.unwrap());

        let driver = MockDeviceDriver::new("unhealthy", true).with_health(false);
        assert!(!driver.health_check().await.unwrap());
    }

    // ── Helper function tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_make_mock_device_registry() {
        let drivers = vec![
            MockDeviceDriver::new("motor", true),
            MockDeviceDriver::new("camera", false),
        ];
        let reg = make_mock_device_registry(drivers);
        assert_eq!(reg.driver_count(), 2);
        let available = reg.probe_all().await.unwrap();
        assert_eq!(available, vec!["motor"]);
    }
}
