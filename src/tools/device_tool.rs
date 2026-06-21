//! Bridge [`Capability`](crate::device::Capability) → [`Tool`](super::Tool).
//!
//! Wraps a device capability as a standard tool so it can be registered in
//! the `ToolRegistry` and dispatched through the Agent's existing tool-calling
//! pipeline — no Agent changes needed.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::device::Capability;
use crate::tools::{Tool, ToolCapabilities, ToolContext, ToolExecutionResult};

/// A [`Tool`] that delegates to a [`Capability`].
///
/// The tool name is `device_{device_id}_{capability_name}` — for example,
/// `device_oscilloscope_01_read_waveform`.  The prefix avoids collisions with
/// built-in tools while keeping the name readable for the LLM.
pub struct DeviceToolWrapper {
    device_id: String,
    name: String,
    description: String,
    capability: Arc<dyn Capability>,
}

impl DeviceToolWrapper {
    /// Create a new wrapper.
    ///
    /// `device_id` should be a short alphanumeric identifier (underscores
    /// allowed).  The tool name is computed as
    /// `device_{device_id}_{capability_name}`.
    pub fn new(device_id: impl Into<String>, capability: Arc<dyn Capability>) -> Self {
        let did = device_id.into();
        let cap_name = capability.name().replace(['.', '-'], "_");
        let name = format!("device_{}_{}", did, cap_name);
        let description = format!("Device '{}' operation: {}", did, capability.name());
        Self {
            device_id: did,
            name,
            description,
            capability,
        }
    }

    /// The raw device identifier.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// A reference to the wrapped capability (for direct access).
    pub fn capability(&self) -> &Arc<dyn Capability> {
        &self.capability
    }
}

#[async_trait]
impl Tool for DeviceToolWrapper {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.capability.param_schema()
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let cap_result = self.capability.execute(args).await;

        let output = cap_result
            .output
            .as_ref()
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => serde_json::to_string(v).ok(),
            })
            .or_else(|| {
                if cap_result.success {
                    Some("OK".to_string())
                } else {
                    cap_result.error.clone()
                }
            })
            .unwrap_or_default();

        Ok(ToolExecutionResult {
            success: cap_result.success,
            output,
            error: cap_result.error,
            data: cap_result.output,
            execution_time: std::time::Duration::from_millis(cap_result.duration_ms),
        })
    }

    /// Device capabilities are high-risk by default — they control physical
    /// hardware.  Users should mark specific devices as trusted in config.
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: true,
            ..ToolCapabilities::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::device::{Capability, CapabilityResult};

    struct DummyMotor;

    #[async_trait]
    impl Capability for DummyMotor {
        fn name(&self) -> &str {
            "motor.move_to"
        }
        fn param_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "position": { "type": "number" }
                }
            })
        }
        async fn execute(&self, params: Value) -> CapabilityResult {
            let pos = params
                .get("position")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            CapabilityResult {
                success: true,
                output: Some(json!({ "position": pos, "status": "moved" })),
                error: None,
                duration_ms: 5,
            }
        }
    }

    #[tokio::test]
    async fn test_tool_name_format() {
        let cap = Arc::new(DummyMotor);
        let wrapper = DeviceToolWrapper::new("stepper_01", cap);
        assert_eq!(wrapper.name(), "device_stepper_01_motor_move_to");
    }

    #[tokio::test]
    async fn test_execute_success() {
        let cap = Arc::new(DummyMotor);
        let wrapper = DeviceToolWrapper::new("stepper_01", cap);
        let ctx = ToolContext::default();
        let result = wrapper
            .execute(json!({ "position": 180.0 }), &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.output.contains("moved"));
        assert!(result.data.is_some());
    }

    #[tokio::test]
    async fn test_parameters_schema() {
        let cap = Arc::new(DummyMotor);
        let wrapper = DeviceToolWrapper::new("stepper_01", cap);
        let schema = wrapper.parameters_schema();
        assert!(schema
            .get("properties")
            .and_then(|p| p.get("position"))
            .is_some());
    }

    #[test]
    fn test_capabilities_requires_approval() {
        let cap = Arc::new(DummyMotor);
        let wrapper = DeviceToolWrapper::new("stepper_01", cap);
        assert!(wrapper.capabilities().requires_approval);
    }
}
