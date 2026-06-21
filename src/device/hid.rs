//! USB HID device driver.
//!
//! Provides [`HidDriver`] for communicating with USB Human Interface Devices
//! (joysticks, gamepads, barcode scanners, etc.).  Feature-gated behind
//! `cfg(feature = "hidapi")`.
//!
//! # Configuration
//!
//! ```json
//! {
//!   "vid": "0x1234",
//!   "pid": "0x5678",
//!   "serial": "A1B2C3",
//!   "usage_page": 1
//! }
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::device::capability::{Capability, CapabilityResult};
use crate::device::driver::DeviceDriver;
use crate::device::safety::{SafetyRule, SafetyRuleKind, SafetyZone};
use crate::device::{Device, DeviceInfo};
use crate::error::Result;

// ── HidDriver
// ─────────────────────────────────────────────────────────────────

/// Driver for USB HID devices.
pub struct HidDriver {
    name: String,
    vid: u16,
    pid: u16,
    serial: Option<String>,
    usage_page: Option<u16>,
}

impl HidDriver {
    /// Create a `HidDriver` from JSON configuration parameters.
    ///
    /// Required: `vid`, `pid`.
    /// Optional: `serial`, `usage_page`, `name`.
    pub fn from_config(params: Value) -> crate::Result<Arc<dyn DeviceDriver>> {
        let vid_str = params.get("vid").and_then(Value::as_str).ok_or_else(|| {
            crate::error::SyscityError::Validation("hid.vid is required (e.g. \"0x1234\")".into())
        })?;

        let pid_str = params.get("pid").and_then(Value::as_str).ok_or_else(|| {
            crate::error::SyscityError::Validation("hid.pid is required (e.g. \"0x5678\")".into())
        })?;

        let vid = u16::from_str_radix(vid_str.trim_start_matches("0x"), 16).map_err(|_| {
            crate::error::SyscityError::Validation(format!("invalid hid.vid: '{}'", vid_str))
        })?;

        let pid = u16::from_str_radix(pid_str.trim_start_matches("0x"), 16).map_err(|_| {
            crate::error::SyscityError::Validation(format!("invalid hid.pid: '{}'", pid_str))
        })?;

        let name = params
            .get("name")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("hid-{:04x}:{:04x}", vid, pid));

        Ok(Arc::new(Self {
            name,
            vid,
            pid,
            serial: params
                .get("serial")
                .and_then(Value::as_str)
                .map(String::from),
            usage_page: params
                .get("usage_page")
                .and_then(Value::as_u64)
                .map(|v| v as u16),
        }))
    }
}

#[async_trait]
impl DeviceDriver for HidDriver {
    fn driver_name(&self) -> &str {
        &self.name
    }

    async fn probe(&self) -> Result<bool> {
        match hidapi::HidApi::new() {
            Ok(api) => {
                for device in api.device_list() {
                    if device.vendor_id() == self.vid && device.product_id() == self.pid {
                        if let Some(ref expected_serial) = self.serial {
                            if let Some(serial_number) = device.serial_number() {
                                if serial_number != expected_serial.as_str() {
                                    continue;
                                }
                            }
                        }
                        if let Some(expected_page) = self.usage_page {
                            if device.usage_page() != expected_page {
                                continue;
                            }
                        }
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Err(e) => {
                tracing::warn!("Failed to initialize HID API: {}", e);
                Ok(false)
            }
        }
    }

    async fn connect(&self) -> Result<Device> {
        let api = hidapi::HidApi::new().map_err(|e| {
            crate::error::SyscityError::Internal(format!("HID API init failed: {}", e))
        })?;

        let handle = api.open(self.vid, self.pid).map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to open HID device: {}", e))
        })?;

        let handle = Arc::new(tokio::sync::Mutex::new(handle));
        let info = DeviceInfo {
            id: self.name.clone(),
            model: format!("HID {:04x}:{:04x}", self.vid, self.pid),
            firmware_version: None,
            location: None,
        };

        let capabilities: Vec<Arc<dyn Capability>> = vec![
            Arc::new(HidReadCapability { handle: handle.clone() }),
            Arc::new(HidWriteCapability { handle }),
        ];

        Ok(Device::new(
            info,
            capabilities,
            SafetyZone::new(vec![SafetyRule {
                kind: SafetyRuleKind::RequiresApproval,
                name: "hid.write".into(),
            }]),
        ))
    }
}

// ── Capabilities
// ──────────────────────────────────────────────────────────────

struct HidReadCapability {
    handle: Arc<tokio::sync::Mutex<hidapi::HidDevice>>,
}

#[async_trait]
impl Capability for HidReadCapability {
    fn name(&self) -> &str {
        "hid.read"
    }

    fn param_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "length": {
                    "type": "integer",
                    "description": "Number of bytes to read (max 64)",
                    "default": 64
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Read timeout in milliseconds",
                    "default": 1000
                }
            }
        })
    }

    async fn execute(&self, params: Value) -> CapabilityResult {
        let length = params.get("length").and_then(Value::as_u64).unwrap_or(64) as usize;
        let length = length.min(64);
        let mut buf = vec![0u8; length];
        let handle = self.handle.lock().await;
        match handle.read_timeout(&mut buf, 1000) {
            Ok(n) => {
                buf.truncate(n);
                CapabilityResult {
                    success: true,
                    output: Some(serde_json::json!({
                        "data": hex::encode(&buf),
                        "length": n,
                    })),
                    error: None,
                    duration_ms: 0,
                }
            }
            Err(e) => CapabilityResult {
                success: false,
                output: None,
                error: Some(format!("HID read error: {}", e)),
                duration_ms: 0,
            },
        }
    }
}

struct HidWriteCapability {
    handle: Arc<tokio::sync::Mutex<hidapi::HidDevice>>,
}

#[async_trait]
impl Capability for HidWriteCapability {
    fn name(&self) -> &str {
        "hid.write"
    }

    fn param_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "string",
                    "description": "Hex-encoded data to write (first byte is report ID)"
                }
            },
            "required": ["data"]
        })
    }

    async fn execute(&self, params: Value) -> CapabilityResult {
        let data_hex = match params.get("data").and_then(Value::as_str) {
            Some(h) => h,
            None => {
                return CapabilityResult {
                    success: false,
                    output: None,
                    error: Some("Missing 'data' parameter".into()),
                    duration_ms: 0,
                };
            }
        };

        let data = match hex::decode(data_hex) {
            Ok(d) => d,
            Err(e) => {
                return CapabilityResult {
                    success: false,
                    output: None,
                    error: Some(format!("Invalid hex data: {}", e)),
                    duration_ms: 0,
                };
            }
        };

        let handle = self.handle.lock().await;
        match handle.write(&data) {
            Ok(n) => CapabilityResult {
                success: true,
                output: Some(serde_json::json!({ "bytes_written": n })),
                error: None,
                duration_ms: 0,
            },
            Err(e) => CapabilityResult {
                success: false,
                output: None,
                error: Some(format!("HID write error: {}", e)),
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
            "vid": "0x1234",
            "pid": "0x5678",
        });
        let driver = HidDriver::from_config(params).expect("should build");
        assert_eq!(driver.driver_name(), "hid-1234:5678");
    }

    #[test]
    fn test_from_config_with_name() {
        let params = json!({
            "vid": "0x1234",
            "pid": "0x5678",
            "name": "barcode-scanner",
        });
        let driver = HidDriver::from_config(params).expect("should build");
        assert_eq!(driver.driver_name(), "barcode-scanner");
    }

    #[test]
    fn test_from_config_missing_vid() {
        let params = json!({ "pid": "0x5678" });
        match HidDriver::from_config(params) {
            Err(e) => assert!(e.to_string().contains("vid"), "error: {}", e),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn test_from_config_missing_pid() {
        let params = json!({ "vid": "0x1234" });
        match HidDriver::from_config(params) {
            Err(e) => assert!(e.to_string().contains("pid"), "error: {}", e),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn test_from_config_invalid_vid() {
        let params = json!({ "vid": "not-hex", "pid": "0x5678" });
        let result = HidDriver::from_config(params);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_config_with_all_options() {
        let params = json!({
            "vid": "0x1234",
            "pid": "0x5678",
            "serial": "A1B2C3",
            "usage_page": 1,
            "name": "custom-hid",
        });
        let driver = HidDriver::from_config(params).expect("should build");
        assert_eq!(driver.driver_name(), "custom-hid");
    }

    #[test]
    fn test_from_config_without_0x_prefix() {
        let params = json!({
            "vid": "1234",
            "pid": "5678",
        });
        let driver = HidDriver::from_config(params).expect("should build");
        assert_eq!(driver.driver_name(), "hid-1234:5678");
    }

    #[test]
    fn test_probe_absent() {
        let params = json!({
            "vid": "0xDEAD",
            "pid": "0xBEEF",
        });
        let driver = HidDriver::from_config(params).expect("should build");
        let present = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(driver.probe())
            .expect("probe should not error");
        assert!(!present);
    }
}
