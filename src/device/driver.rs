//! Device driver trait — the boundary between Syscity and physical hardware.
//!
//! Each physical device type (motor, camera, sensor, etc.) implements
//! [`DeviceDriver`] to provide probe, connect, and lifecycle management.

use crate::device::Device;
use crate::error::Result;
use serde_json::Value;

/// Optional lifecycle operations for advanced device management.
///
/// Drivers that implement self-test, calibration, firmware update, or
/// configuration read/write can provide this trait alongside
/// [`DeviceDriver`].  Use [`DeviceDriver::as_lifecycle`] to upcast.
///
/// All methods have default no-op implementations so implementors only
/// override what they support.
#[async_trait::async_trait]
pub trait DeviceLifecycle: Send + Sync {
    /// Run the device's built-in self-test.
    async fn self_test(&self) -> Result<bool> {
        Ok(true)
    }

    /// Calibrate the device with the given parameters.
    async fn calibrate(&self, _params: Value) -> Result<()> {
        Ok(())
    }

    /// Update the device firmware.
    async fn update_firmware(&self, _firmware: Vec<u8>) -> Result<()> {
        Ok(())
    }

    /// Read the current device configuration.
    async fn read_config(&self) -> Result<Value> {
        Ok(Value::Null)
    }

    /// Write a new device configuration.
    async fn write_config(&self, _config: Value) -> Result<()> {
        Ok(())
    }
}
///
/// Drivers are responsible for:
/// - Probing whether the hardware is present (`probe`)
/// - Establishing a connection and building the [`Device`] object (`connect`)
/// - Providing a stable name for identification (`driver_name`)
///
/// # Example
///
/// ```ignore
/// use syscity::device::{Device, DeviceDriver};
///
/// struct MyMotorDriver;
///
/// #[async_trait::async_trait]
/// impl DeviceDriver for MyMotorDriver {
///     fn driver_name(&self) -> &str { "nema17-stepper" }
///
///     async fn probe(&self) -> Result<bool> {
///         Ok(true) // hardware detected
///     }
///
///     async fn connect(&self) -> Result<Device> {
///         // build Device with capabilities
///         todo!()
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait DeviceDriver: Send + Sync {
    /// Human-readable driver name, e.g. `"nema17-stepper"`, `"uvc-camera"`.
    fn driver_name(&self) -> &str;

    /// Probe for the physical device.
    ///
    /// Returns `Ok(true)` if the device is present and ready for connection,
    /// `Ok(false)` if absent. Returns `Err` on probe errors.
    async fn probe(&self) -> Result<bool>;

    /// Connect to the device and build the [`Device`] object.
    ///
    /// This should register all [`Capability`](crate::device::Capability)
    /// implementations for the device's operations and attach a
    /// [`SafetyZone`](crate::device::SafetyZone) with appropriate
    /// rules.
    ///
    /// # Errors
    ///
    /// Returns an error if the device cannot be initialized.
    async fn connect(&self) -> Result<Device>;

    /// Optional: disconnect / release hardware resources.
    async fn disconnect(&self) -> Result<()> {
        Ok(())
    }

    /// Optional: perform a health check on the connected device.
    ///
    /// Returns `Ok(true)` if healthy, `Ok(false)` if degraded/unreachable.
    async fn health_check(&self) -> Result<bool> {
        Ok(true)
    }

    /// Upcast to [`DeviceLifecycle`] if the driver supports it.
    ///
    /// Returns `None` by default. Override to return `Some(&self)`.
    fn as_lifecycle(&self) -> Option<&dyn DeviceLifecycle> {
        None
    }
}
