//! Serial port device driver.
//!
//! Provides [`SerialPortDriver`] for communicating with devices over RS-232
//! / USB-serial adapters.  Feature-gated behind `cfg(feature = "serialport")`.
//!
//! # Configuration
//!
//! ```json
//! {
//!   "path": "/dev/ttyUSB0",
//!   "baud_rate": 115200,
//!   "data_bits": 8,
//!   "stop_bits": "1",
//!   "parity": "none"
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

// ── SerialPortDriver
// ──────────────────────────────────────────────────────────

/// Driver for serial port devices.
pub struct SerialPortDriver {
    name: String,
    path: String,
    baud_rate: u32,
    data_bits: serialport::DataBits,
    stop_bits: serialport::StopBits,
    parity: serialport::Parity,
}

fn parse_data_bits(val: &Value) -> crate::Result<serialport::DataBits> {
    let n = val.as_u64().unwrap_or(8);
    match n {
        5 => Ok(serialport::DataBits::Five),
        6 => Ok(serialport::DataBits::Six),
        7 => Ok(serialport::DataBits::Seven),
        8 => Ok(serialport::DataBits::Eight),
        other => Err(crate::error::SyscityError::Validation(format!(
            "serialport.data_bits: invalid value '{}' (expected 5, 6, 7, or 8)",
            other
        ))),
    }
}

fn parse_stop_bits(val: &Value) -> crate::Result<serialport::StopBits> {
    match val.as_str().unwrap_or("1") {
        "1" => Ok(serialport::StopBits::One),
        "2" => Ok(serialport::StopBits::Two),
        other => Err(crate::error::SyscityError::Validation(format!(
            "serialport.stop_bits: invalid value '{}' (expected 1 or 2)",
            other
        ))),
    }
}

fn parse_parity(val: &Value) -> crate::Result<serialport::Parity> {
    match val.as_str().unwrap_or("none") {
        "none" => Ok(serialport::Parity::None),
        "odd" => Ok(serialport::Parity::Odd),
        "even" => Ok(serialport::Parity::Even),
        other => Err(crate::error::SyscityError::Validation(format!(
            "serialport.parity: invalid value '{}' (expected none, odd, or even)",
            other
        ))),
    }
}

impl SerialPortDriver {
    /// Create a `SerialPortDriver` from JSON configuration parameters.
    ///
    /// Required: `path`, `baud_rate`.
    /// Optional: `data_bits` (default 8), `stop_bits` (default `"1"`),
    /// `parity` (default `"none"`).
    pub fn from_config(params: Value) -> crate::Result<Arc<dyn DeviceDriver>> {
        let path = params.get("path").and_then(Value::as_str).ok_or_else(|| {
            crate::error::SyscityError::Validation("serialport.path is required".into())
        })?;

        let baud_rate = params
            .get("baud_rate")
            .and_then(Value::as_u64)
            .unwrap_or(9600) as u32;

        let data_bits =
            parse_data_bits(params.get("data_bits").unwrap_or(&Value::Number(8.into())))?;
        let stop_bits = parse_stop_bits(
            params
                .get("stop_bits")
                .unwrap_or(&Value::String("1".into())),
        )?;
        let parity = parse_parity(
            params
                .get("parity")
                .unwrap_or(&Value::String("none".into())),
        )?;

        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(path)
            .to_string();

        Ok(Arc::new(Self {
            name,
            path: path.to_string(),
            baud_rate,
            data_bits,
            stop_bits,
            parity,
        }))
    }

    fn device_name(&self) -> &str {
        Path::new(&self.path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.path)
    }
}

#[async_trait]
impl DeviceDriver for SerialPortDriver {
    fn driver_name(&self) -> &str {
        &self.name
    }

    async fn probe(&self) -> Result<bool> {
        Ok(Path::new(&self.path).exists())
    }

    async fn connect(&self) -> Result<Device> {
        let mut builder = serialport::new(&self.path, self.baud_rate);
        builder = builder
            .data_bits(self.data_bits)
            .stop_bits(self.stop_bits)
            .parity(self.parity);

        let port = builder.open().map_err(|e| {
            crate::error::SyscityError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                format!("{}: {}", self.name, e),
            ))
        })?;

        let port_inner = Arc::new(tokio::sync::Mutex::new(port));
        let info = DeviceInfo {
            id: self.name.clone(),
            model: format!("SerialPort ({})", self.device_name()),
            firmware_version: None,
            location: Some(self.path.clone()),
        };

        let capabilities: Vec<Arc<dyn Capability>> = vec![
            Arc::new(SerialReadCapability { port: port_inner.clone() }),
            Arc::new(SerialWriteCapability { port: port_inner }),
        ];

        Ok(Device::new(
            info,
            capabilities,
            SafetyZone::new(vec![SafetyRule {
                kind: SafetyRuleKind::RequiresApproval,
                name: "serial.write".into(),
            }]),
        ))
    }
}

// ── Capabilities
// ──────────────────────────────────────────────────────────────

struct SerialReadCapability {
    port: Arc<tokio::sync::Mutex<Box<dyn serialport::SerialPort>>>,
}

#[async_trait]
impl Capability for SerialReadCapability {
    fn name(&self) -> &str {
        "serial.read"
    }

    fn param_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "length": {
                    "type": "integer",
                    "description": "Number of bytes to read",
                    "default": 256
                }
            }
        })
    }

    async fn execute(&self, params: Value) -> CapabilityResult {
        let length = params.get("length").and_then(Value::as_u64).unwrap_or(256) as usize;
        let mut buf = vec![0u8; length];
        let mut port = self.port.lock().await;
        match port.read(&mut buf) {
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
                error: Some(format!("Serial read error: {}", e)),
                duration_ms: 0,
            },
        }
    }
}

struct SerialWriteCapability {
    port: Arc<tokio::sync::Mutex<Box<dyn serialport::SerialPort>>>,
}

#[async_trait]
impl Capability for SerialWriteCapability {
    fn name(&self) -> &str {
        "serial.write"
    }

    fn param_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "data": {
                    "type": "string",
                    "description": "Hex-encoded data to write (e.g. \"ff01a2\")"
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

        let mut port = self.port.lock().await;
        match port.write(&data) {
            Ok(n) => CapabilityResult {
                success: true,
                output: Some(serde_json::json!({ "bytes_written": n })),
                error: None,
                duration_ms: 0,
            },
            Err(e) => CapabilityResult {
                success: false,
                output: None,
                error: Some(format!("Serial write error: {}", e)),
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
            "path": "/dev/ttyUSB0",
            "baud_rate": 115200,
        });
        let driver =
            SerialPortDriver::from_config(params).expect("should build from minimal config");
        assert_eq!(driver.driver_name(), "/dev/ttyUSB0");
    }

    #[test]
    fn test_from_config_with_name() {
        let params = json!({
            "name": "sensor-port",
            "path": "/dev/ttyUSB0",
            "baud_rate": 9600,
        });
        let driver = SerialPortDriver::from_config(params).expect("should build");
        assert_eq!(driver.driver_name(), "sensor-port");
    }

    #[test]
    fn test_from_config_missing_path() {
        let params = json!({ "baud_rate": 115200 });
        match SerialPortDriver::from_config(params) {
            Err(e) => assert!(e.to_string().contains("path"), "error: {}", e),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn test_from_config_invalid_stop_bits() {
        let params = json!({
            "path": "/dev/ttyUSB0",
            "stop_bits": "3",
        });
        let result = SerialPortDriver::from_config(params);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_config_invalid_parity() {
        let params = json!({
            "path": "/dev/ttyUSB0",
            "parity": "invalid",
        });
        let result = SerialPortDriver::from_config(params);
        assert!(result.is_err());
    }

    #[test]
    fn test_probe_absent() {
        let params = json!({
            "path": "/dev/nonexistent-serial-test",
            "baud_rate": 9600,
        });
        let driver = SerialPortDriver::from_config(params).expect("should build");
        let present = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(driver.probe())
            .expect("probe should not error");
        assert!(!present, "nonexistent device should not be present");
    }

    #[test]
    fn test_from_config_all_options() {
        let params = json!({
            "name": "full-config",
            "path": "/dev/ttyS0",
            "baud_rate": 57600,
            "data_bits": 7,
            "stop_bits": "2",
            "parity": "even",
        });
        let driver = SerialPortDriver::from_config(params).expect("should build with all options");
        assert_eq!(driver.driver_name(), "full-config");
    }

    #[test]
    fn test_from_config_invalid_data_bits() {
        let params = json!({
            "path": "/dev/ttyUSB0",
            "data_bits": 9,
        });
        let result = SerialPortDriver::from_config(params);
        assert!(result.is_err());
    }
}
