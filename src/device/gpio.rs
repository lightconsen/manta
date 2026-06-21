//! Linux GPIO device driver (sysfs).
//!
//! Provides [`GpioDriver`] for controlling GPIO pins via the Linux sysfs
//! interface (`/sys/class/gpio`).  Only available on Linux targets.
//!
//! # Configuration
//!
//! ```json
//! {
//!   "pins": [17, 22, 27],
//!   "name": "lab-gpio"
//! }
//! ```

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::device::capability::{Capability, CapabilityResult};
use crate::device::driver::DeviceDriver;
use crate::device::safety::{SafetyRule, SafetyRuleKind, SafetyZone};
use crate::device::{Device, DeviceInfo};
use crate::error::Result;

// ── GpioDriver
// ────────────────────────────────────────────────────────────────

/// Driver for Linux sysfs GPIO devices.
///
/// Exports pins via `/sys/class/gpio/export` and provides read/write/set_mode
/// capabilities.  Each pin is cleaned up (unexported) on disconnect.
pub struct GpioDriver {
    name: String,
    pins: Vec<u32>,
}

impl GpioDriver {
    /// Create a `GpioDriver` from JSON configuration parameters.
    ///
    /// Required: `pins` (array of pin numbers).
    /// Optional: `name`.
    pub fn from_config(params: Value) -> crate::Result<Arc<dyn DeviceDriver>> {
        let pins_array = params
            .get("pins")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                crate::error::SyscityError::Validation("gpio.pins is required".into())
            })?;

        if pins_array.is_empty() {
            return Err(crate::error::SyscityError::Validation(
                "gpio.pins must not be empty".into(),
            ));
        }

        let pins: Vec<u32> = pins_array
            .iter()
            .filter_map(|v| v.as_u64().map(|n| n as u32))
            .collect();

        if pins.len() != pins_array.len() {
            return Err(crate::error::SyscityError::Validation(
                "gpio.pins: all pins must be unsigned integers".into(),
            ));
        }

        let name = params
            .get("name")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                let pin_strs: Vec<String> = pins.iter().map(|p| p.to_string()).collect();
                format!("gpio-{}", pin_strs.join("_"))
            });

        Ok(Arc::new(Self { name, pins }))
    }

    /// Path to the sysfs GPIO class directory.
    fn gpio_class_path() -> &'static Path {
        Path::new("/sys/class/gpio")
    }

    /// Export a GPIO pin via sysfs.
    fn export_pin(pin: u32) -> std::io::Result<()> {
        let gpio_path = Self::gpio_class_path().join(format!("gpio{}", pin));
        if gpio_path.exists() {
            return Ok(()); // Already exported
        }
        std::fs::write(Self::gpio_class_path().join("export"), pin.to_string())
    }

    /// Unexport a GPIO pin via sysfs.
    fn unexport_pin(pin: u32) -> std::io::Result<()> {
        std::fs::write(Self::gpio_class_path().join("unexport"), pin.to_string())
    }

    /// Set the direction of a GPIO pin.
    fn set_pin_direction(pin: u32, direction: &str) -> std::io::Result<()> {
        std::fs::write(Self::gpio_class_path().join(format!("gpio{}/direction", pin)), direction)
    }

    /// Read the value of a GPIO pin.
    fn read_pin(pin: u32) -> std::io::Result<bool> {
        let val =
            std::fs::read_to_string(Self::gpio_class_path().join(format!("gpio{}/value", pin)))?;
        Ok(val.trim() == "1")
    }

    /// Write a value to a GPIO pin.
    fn write_pin(pin: u32, value: bool) -> std::io::Result<()> {
        std::fs::write(
            Self::gpio_class_path().join(format!("gpio{}/value", pin)),
            if value { "1" } else { "0" },
        )
    }
}

#[async_trait]
impl DeviceDriver for GpioDriver {
    fn driver_name(&self) -> &str {
        &self.name
    }

    async fn probe(&self) -> Result<bool> {
        Ok(Self::gpio_class_path().exists())
    }

    async fn connect(&self) -> Result<Device> {
        // Export all pins
        for &pin in &self.pins {
            if let Err(e) = Self::export_pin(pin) {
                tracing::warn!("Failed to export GPIO pin {}: {}", pin, e);
            }
            // Set default direction to "in"
            let _ = Self::set_pin_direction(pin, "in");
        }

        let pins = self.pins.clone();
        let info = DeviceInfo {
            id: self.name.clone(),
            model: format!("GPIO ({} pins)", self.pins.len()),
            firmware_version: None,
            location: Some("/sys/class/gpio".into()),
        };

        let capabilities: Vec<Arc<dyn Capability>> = vec![
            Arc::new(GpioReadCapability { pins: pins.clone() }),
            Arc::new(GpioWriteCapability { pins: pins.clone() }),
            Arc::new(GpioSetModeCapability { pins }),
        ];

        Ok(Device::new(
            info,
            capabilities,
            SafetyZone::new(vec![SafetyRule {
                kind: SafetyRuleKind::RequiresApproval,
                name: "gpio.write".into(),
            }]),
        ))
    }

    async fn disconnect(&self) -> Result<()> {
        for &pin in &self.pins {
            let _ = Self::unexport_pin(pin);
        }
        Ok(())
    }
}

// ── Capabilities
// ──────────────────────────────────────────────────────────────

struct GpioReadCapability {
    pins: Vec<u32>,
}

#[async_trait]
impl Capability for GpioReadCapability {
    fn name(&self) -> &str {
        "gpio.read"
    }

    fn param_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "Pin number to read. Reads all pins if omitted."
                }
            }
        })
    }

    async fn execute(&self, params: Value) -> CapabilityResult {
        let specific_pin = params.get("pin").and_then(Value::as_u64);

        let read_pin_fn = |pin: u32| -> Option<bool> { GpioDriver::read_pin(pin).ok() };

        match specific_pin {
            Some(pin_num) => {
                let pin = pin_num as u32;
                if !self.pins.contains(&pin) {
                    return CapabilityResult {
                        success: false,
                        output: None,
                        error: Some(format!("Pin {} is not managed by this driver", pin)),
                        duration_ms: 0,
                    };
                }
                let value = read_pin_fn(pin);
                CapabilityResult {
                    success: value.is_some(),
                    output: Some(serde_json::json!({
                        "pin": pin,
                        "value": value,
                    })),
                    error: value
                        .map_or_else(|| Some(format!("Failed to read pin {}", pin)), |_| None),
                    duration_ms: 0,
                }
            }
            None => {
                let mut results = serde_json::Map::new();
                for &pin in &self.pins {
                    if let Some(val) = read_pin_fn(pin) {
                        results.insert(pin.to_string(), serde_json::json!(val));
                    }
                }
                CapabilityResult {
                    success: true,
                    output: Some(serde_json::json!({ "pins": results })),
                    error: None,
                    duration_ms: 0,
                }
            }
        }
    }
}

struct GpioWriteCapability {
    pins: Vec<u32>,
}

#[async_trait]
impl Capability for GpioWriteCapability {
    fn name(&self) -> &str {
        "gpio.write"
    }

    fn param_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "Pin number to write to"
                },
                "value": {
                    "type": "boolean",
                    "description": "Value to write (true=high, false=low)"
                }
            },
            "required": ["pin", "value"]
        })
    }

    async fn execute(&self, params: Value) -> CapabilityResult {
        let pin = match params.get("pin").and_then(Value::as_u64) {
            Some(p) => p as u32,
            None => {
                return CapabilityResult {
                    success: false,
                    output: None,
                    error: Some("Missing 'pin' parameter".into()),
                    duration_ms: 0,
                };
            }
        };

        if !self.pins.contains(&pin) {
            return CapabilityResult {
                success: false,
                output: None,
                error: Some(format!("Pin {} is not managed by this driver", pin)),
                duration_ms: 0,
            };
        }

        let value = params
            .get("value")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // Ensure pin direction is set to "out" before writing
        let _ = GpioDriver::set_pin_direction(pin, "out");

        match GpioDriver::write_pin(pin, value) {
            Ok(()) => CapabilityResult {
                success: true,
                output: Some(serde_json::json!({ "pin": pin, "value": value })),
                error: None,
                duration_ms: 0,
            },
            Err(e) => CapabilityResult {
                success: false,
                output: None,
                error: Some(format!("GPIO write error on pin {}: {}", pin, e)),
                duration_ms: 0,
            },
        }
    }
}

struct GpioSetModeCapability {
    pins: Vec<u32>,
}

#[async_trait]
impl Capability for GpioSetModeCapability {
    fn name(&self) -> &str {
        "gpio.set_mode"
    }

    fn param_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pin": {
                    "type": "integer",
                    "description": "Pin number"
                },
                "mode": {
                    "type": "string",
                    "enum": ["in", "out"],
                    "description": "Pin direction: 'in' for input, 'out' for output"
                }
            },
            "required": ["pin", "mode"]
        })
    }

    async fn execute(&self, params: Value) -> CapabilityResult {
        let pin = match params.get("pin").and_then(Value::as_u64) {
            Some(p) => p as u32,
            None => {
                return CapabilityResult {
                    success: false,
                    output: None,
                    error: Some("Missing 'pin' parameter".into()),
                    duration_ms: 0,
                };
            }
        };

        if !self.pins.contains(&pin) {
            return CapabilityResult {
                success: false,
                output: None,
                error: Some(format!("Pin {} is not managed by this driver", pin)),
                duration_ms: 0,
            };
        }

        let mode = match params.get("mode").and_then(Value::as_str) {
            Some(m) if m == "in" || m == "out" => m,
            Some(other) => {
                return CapabilityResult {
                    success: false,
                    output: None,
                    error: Some(format!("Invalid mode '{}'. Use 'in' or 'out'", other)),
                    duration_ms: 0,
                };
            }
            None => {
                return CapabilityResult {
                    success: false,
                    output: None,
                    error: Some("Missing 'mode' parameter".into()),
                    duration_ms: 0,
                };
            }
        };

        match GpioDriver::set_pin_direction(pin, mode) {
            Ok(()) => CapabilityResult {
                success: true,
                output: Some(serde_json::json!({ "pin": pin, "mode": mode })),
                error: None,
                duration_ms: 0,
            },
            Err(e) => CapabilityResult {
                success: false,
                output: None,
                error: Some(format!("Failed to set mode on pin {}: {}", pin, e)),
                duration_ms: 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_from_config_minimal() {
        let params = json!({
            "pins": [17, 22],
        });
        let driver = GpioDriver::from_config(params).expect("should build");
        assert_eq!(driver.driver_name(), "gpio-17_22");
    }

    #[test]
    fn test_from_config_with_name() {
        let params = json!({
            "pins": [17, 22, 27],
            "name": "lab-gpio",
        });
        let driver = GpioDriver::from_config(params).expect("should build");
        assert_eq!(driver.driver_name(), "lab-gpio");
    }

    #[test]
    fn test_from_config_missing_pins() {
        let params = json!({});
        let result = GpioDriver::from_config(params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("pins"));
    }

    #[test]
    fn test_from_config_empty_pins() {
        let params = json!({ "pins": [] });
        let result = GpioDriver::from_config(params);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_config_invalid_pin_type() {
        let params = json!({ "pins": ["not-a-number"] });
        // All entries will be filtered out, so the result will be empty pins
        let result = GpioDriver::from_config(params);
        assert!(result.is_err());
    }

    #[test]
    fn test_single_pin() {
        let params = json!({
            "pins": [4],
            "name": "single-pin",
        });
        let driver = GpioDriver::from_config(params).expect("should build");
        assert_eq!(driver.driver_name(), "single-pin");
    }

    #[test]
    fn test_probe_non_linux() {
        // On non-Linux systems this will return false.
        // On Linux without /sys/class/gpio, this returns false.
        // The test just verifies no panic and returns a boolean.
        let params = json!({ "pins": [17] });
        let driver = GpioDriver::from_config(params).expect("should build");
        let present = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(driver.probe())
            .expect("probe should not error");
        // Just verify it's a boolean; on most test systems /sys/class/gpio won't exist
        assert!(!present || std::path::Path::new("/sys/class/gpio").exists());
    }
}
