//! Bridge [`Device`](crate::device::Device) → [`Capability`](super::super::Capability).
//!
//! Each individual operation exposed by a physical device is already a
//! [`Capability`]; this module provides helpers to extract and register
//! device capabilities into a [`CapabilityRegistry`].

use crate::capability::registry::CapabilityRegistry;
use crate::device::Device;
use std::sync::Arc;

/// Register all capabilities from `device` into `registry`.
///
/// Each capability in `device.capabilities` gets registered using its
/// `name()` as the key.
///
/// ```ignore
/// use syscity::capability::registry::CapabilityRegistry;
/// use syscity::capability::providers::device_adapter;
/// use std::sync::Arc;
///
/// let mut reg = CapabilityRegistry::new();
/// let device: Arc<Device> = /* ... */;
/// device_adapter::register_device_capabilities(&mut reg, device);
/// ```
pub fn register_device_capabilities(
    registry: &mut CapabilityRegistry,
    device: Arc<Device>,
) {
    for cap in &device.capabilities {
        registry.register_or_replace(cap.clone());
    }
}

/// Register all capabilities from multiple devices.
pub fn register_devices(
    registry: &mut CapabilityRegistry,
    devices: Vec<Arc<Device>>,
) {
    for device in devices {
        register_device_capabilities(registry, device);
    }
}

/// Collect all capability names exposed by a device.
pub fn device_capability_names(device: &Device) -> Vec<String> {
    let mut names: Vec<String> = device
        .capabilities
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::safety::{SafetyRule, SafetyRuleKind, SafetyZone};
    use crate::capability::{Capability, CapabilityResult};
    use crate::device::DeviceInfo;
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;

    struct MockMotorCap;

    #[async_trait]
    impl Capability for MockMotorCap {
        fn name(&self) -> &str {
            "motor.move_to"
        }
        fn param_schema(&self) -> Value {
            Value::Null
        }
        async fn execute(&self, _params: Value) -> CapabilityResult {
            CapabilityResult {
                success: true,
                output: None,
                error: None,
                duration_ms: 0,
            }
        }
    }

    struct MockCameraCap;

    #[async_trait]
    impl Capability for MockCameraCap {
        fn name(&self) -> &str {
            "camera.capture"
        }
        fn param_schema(&self) -> Value {
            Value::Null
        }
        async fn execute(&self, _params: Value) -> CapabilityResult {
            CapabilityResult {
                success: true,
                output: None,
                error: None,
                duration_ms: 0,
            }
        }
    }

    fn make_test_device() -> Arc<Device> {
        let info = DeviceInfo {
            id: "dev-01".into(),
            model: "Test".into(),
            firmware_version: None,
            location: None,
        };
        Arc::new(Device::new(
            info,
            vec![
                Arc::new(MockMotorCap) as Arc<dyn Capability>,
                Arc::new(MockCameraCap) as Arc<dyn Capability>,
            ],
            SafetyZone::new(vec![SafetyRule {
                name: "default".into(),
                kind: SafetyRuleKind::RequiresApproval,
            }]),
        ))
    }

    #[test]
    fn test_device_capability_names() {
        let device = make_test_device();
        let names = device_capability_names(&device);
        assert_eq!(names, vec!["camera.capture", "motor.move_to"]);
    }

    #[test]
    fn test_register_device_capabilities() {
        let device = make_test_device();
        let mut reg = CapabilityRegistry::new();
        register_device_capabilities(&mut reg, device);

        assert!(reg.resolve("motor.move_to").is_some());
        assert!(reg.resolve("camera.capture").is_some());
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn test_register_devices() {
        let device = make_test_device();
        let mut reg = CapabilityRegistry::new();
        register_devices(&mut reg, vec![device]);

        assert!(reg.resolve("motor.move_to").is_some());
    }
}
