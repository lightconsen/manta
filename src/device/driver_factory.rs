//! Pluggable device driver factory.
//!
//! [`DriverFactory`] maps config-level driver `kind` strings (e.g. `"mock"`,
//! `"serialport"`) to constructor functions that produce [`DeviceDriver`]
//! instances from JSON parameters.
//!
//! # Architecture
//!
//! The factory is stored in [`GatewayState`](crate::gateway::GatewayState) and
//! shared between the static config-driver discovery path and the runtime OS
//! bridge event path.  Because the inner registry is behind [`Arc`]`<`[`RwLock`]
//! `>`, the factory can be cloned and drivers registered at any point — during
//! startup, from native plugins, or programmatically.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[cfg(feature = "native-plugins")]
use std::path::Path;

use serde_json::Value;

use crate::device::DeviceDriver;
use crate::error::SyscityError;

/// Constructor signature — builds a device driver from JSON parameters.
///
/// Uses [`Arc`]`<`[`dyn Fn`]`>` instead of a bare `fn` pointer so that
/// plugin-loaded constructors (e.g. via `libloading`) can capture the loaded
/// library handle.
pub type DriverConstructor =
    Arc<dyn Fn(Value) -> crate::Result<Arc<dyn DeviceDriver>> + Send + Sync>;

/// Registry of driver constructors keyed by their config `kind` string.
///
/// The inner map is behind [`Arc`]`<`[`RwLock`]`>` so the factory can be
/// cloned and drivers can be registered at any time without `&mut` access.
///
/// # Example
///
/// ```ignore
/// let factory = DriverFactory::new();
/// let driver = factory.build("mock", json!({ "name": "sensor" }))?;
/// ```
#[derive(Clone)]
pub struct DriverFactory {
    inner: Arc<RwLock<HashMap<String, DriverConstructor>>>,
}

impl DriverFactory {
    /// Create a factory with all built-in driver constructors registered.
    pub fn new() -> Self {
        let factory = Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        };
        factory.register_fn("mock", crate::device::mock::MockDeviceDriver::from_config);
        #[cfg(feature = "serialport")]
        factory.register_fn("serialport", crate::device::serialport::SerialPortDriver::from_config);
        #[cfg(feature = "hidapi")]
        factory.register_fn("hid", crate::device::hid::HidDriver::from_config);
        #[cfg(target_os = "linux")]
        factory.register_fn("gpio", crate::device::gpio::GpioDriver::from_config);
        factory
    }

    /// Register a driver constructor for the given `kind` string.
    pub fn register(&self, kind: &str, ctor: DriverConstructor) {
        if let Ok(mut map) = self.inner.write() {
            map.insert(kind.to_string(), ctor);
        }
    }

    /// Register a driver constructor from a bare `fn` pointer.
    ///
    /// Convenience wrapper that wraps the function pointer into an
    /// [`Arc`]`<`[`dyn Fn`]`>`.
    pub fn register_fn(
        &self,
        kind: &str,
        ctor: fn(Value) -> crate::Result<Arc<dyn DeviceDriver>>,
    ) {
        self.register(kind, Arc::new(ctor));
    }

    /// Build a driver by `kind`, passing `params` to its constructor.
    ///
    /// Returns an error if `kind` is not registered.
    pub fn build(&self, kind: &str, params: Value) -> crate::Result<Arc<dyn DeviceDriver>> {
        let map = self.inner.read().map_err(|_| SyscityError::Internal(
            "DriverFactory lock poisoned".into(),
        ))?;
        let ctor = map.get(kind).ok_or_else(|| SyscityError::NotFound {
            resource: format!("Device driver kind '{}'", kind),
        })?;
        ctor(params)
    }

    /// Check if a driver kind is registered.
    pub fn has_kind(&self, kind: &str) -> bool {
        self.inner.read().ok().is_some_and(|map| map.contains_key(kind))
    }

    /// List all registered driver kinds.
    pub fn kinds(&self) -> Vec<String> {
        self.inner.read().ok().map_or_else(Vec::new, |map| {
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort();
            keys
        })
    }

    /// Load and register a driver from a native plugin shared library at
    /// the given path.
    ///
    /// The `load` and `into_constructor` operations are contained in the
    /// `native_plugin` module where `unsafe` is allowed.
    #[cfg(feature = "native-plugins")]
    pub fn register_native_plugin(&self, path: &Path) -> crate::Result<()> {
        let loader = crate::device::native_plugin::NativeDriverLoader::load(path)?;
        let kind = loader.kind().to_string();
        let ctor = loader.into_constructor();
        self.register(&kind, ctor);
        tracing::info!(
            "Registered native plugin driver '{}' from {:?}",
            kind,
            path,
        );
        Ok(())
    }

    /// Scan a directory for native plugin shared libraries and register any
    /// found.
    #[cfg(feature = "native-plugins")]
    pub fn scan_native_plugins_dir(&self, dir: &Path) {
        let plugins = crate::device::native_plugin::scan_native_plugins(dir);
        for (kind, ctor) in plugins {
            self.register(&kind, ctor);
        }
    }
}

impl Default for DriverFactory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::mock::MockDeviceDriver;
    use serde_json::json;

    #[test]
    fn test_build_mock() {
        let factory = DriverFactory::new();
        let driver = factory
            .build("mock", json!({ "name": "cfg-motor", "present": true }))
            .expect("mock driver should build");
        assert_eq!(driver.driver_name(), "cfg-motor");
    }

    #[test]
    fn test_build_unknown() {
        let factory = DriverFactory::new();
        let result = factory.build("nonexistent", json!({}));
        match result {
            Ok(_) => panic!("expected error for unknown driver kind"),
            Err(e) => assert!(e.to_string().contains("nonexistent")),
        }
    }

    #[test]
    fn test_build_mock_defaults() {
        let factory = DriverFactory::new();
        let driver = factory
            .build("mock", json!({}))
            .expect("mock driver with empty params should build");
        assert_eq!(driver.driver_name(), "mock");
    }

    #[test]
    fn test_register_custom() {
        let factory = DriverFactory::new();
        factory.register_fn("custom", |params| {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("custom");
            Ok(Arc::new(MockDeviceDriver::new(name, true)))
        });
        assert!(factory.has_kind("custom"));
        let driver = factory.build("custom", json!({ "name": "my-dev" })).unwrap();
        assert_eq!(driver.driver_name(), "my-dev");
    }

    #[test]
    fn test_clone_shares_state() {
        let f1 = DriverFactory::new();
        let f2 = f1.clone();
        f2.register_fn("extra", |_| Ok(Arc::new(MockDeviceDriver::new("extra", true))));
        assert!(f1.has_kind("extra"), "clone should share state");
    }

    #[test]
    fn test_kinds_list() {
        let factory = DriverFactory::new();
        let kinds = factory.kinds();
        assert!(kinds.contains(&"mock".to_string()));
    }

    #[test]
    fn test_kinds_empty() {
        let factory = DriverFactory {
            inner: Arc::new(RwLock::new(HashMap::new())),
        };
        assert!(factory.kinds().is_empty());
    }
}
