//! Native plugin loading for device drivers.
//!
//! Provides [`NativeDriverLoader`] for loading device drivers from shared
//! libraries (`.so`, `.dylib`, `.dll`) at runtime.  Feature-gated behind
//! `cfg(feature = "native-plugins")`.
//!
//! # C ABI
//!
//! Each shared library must export the following `extern "C"` functions:
//!
//! | Function | Signature | Description |
//! |---|---|---|
//! | `syscity_driver_kind` | `() -> *const c_char` | Returns a null-terminated UTF-8 string identifying the driver kind (e.g. `"my-sensor"`). |
//! | `syscity_driver_create` | `(params: *const c_char) -> *mut c_void` | Allocates a driver from a JSON params string. Returns an opaque pointer. |
//! | `syscity_driver_free` | `(ptr: *mut c_void)` | Deallocates a driver previously returned by `syscity_driver_create`. |
//!
//! # Safety
//!
//! The plugin and the host must be compiled with the same Rust compiler
//! version because the opaque pointer returned by `syscity_driver_create`
//! is cast from/to `Box<dyn DeviceDriver>`, and trait object layout is
//! not stable across compiler versions.
//!
//! This entire module is marked `#[allow(unsafe_code)]` — the unsafety is
//! confined to the FFI boundary functions.

#![allow(unsafe_code)]

use std::ffi::{c_char, CStr, CString};
use std::path::Path;
use std::sync::Arc;

use libloading::{Library, Symbol};
use serde_json::Value;

use crate::device::driver::DeviceDriver;
use crate::device::DriverConstructor;
use crate::error::Result;

// ── C ABI function signatures ────────────────────────────────────────────────

/// C ABI: `syscity_driver_kind() -> *const c_char`
type KindFn = unsafe extern "C" fn() -> *const c_char;

/// C ABI: `syscity_driver_create(params: *const c_char) -> *mut c_void`
type CreateFn = unsafe extern "C" fn(*const c_char) -> *mut std::ffi::c_void;

/// C ABI: `syscity_driver_free(ptr: *mut c_void)`
type FreeFn = unsafe extern "C" fn(*mut std::ffi::c_void);

// ── NativeDriverLoader
// ────────────────────────────────────────────────────────

/// Loads device drivers from native shared libraries.
///
/// # Example
///
/// ```ignore
/// let loader = NativeDriverLoader::load("/path/to/libmysensor.so")?;
/// assert_eq!(loader.kind(), "my-sensor");
/// let constructor = loader.into_constructor();
/// ```
pub struct NativeDriverLoader {
    /// Keep the library alive for the lifetime of the loader.
    _lib: Library,
    kind: String,
    create_fn: CreateFn,
    free_fn: FreeFn,
}

impl NativeDriverLoader {
    /// Load a device driver from a shared library at `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        // SAFETY: Library::new loads a shared library into the process
        // address space.  Unsafe because the library's init code runs
        // immediately and may have arbitrary side effects.
        let lib = unsafe {
            Library::new(path.as_ref()).map_err(|e| {
                crate::error::SyscityError::Plugin(format!(
                    "Failed to load native plugin '{}': {}",
                    path.as_ref().display(),
                    e,
                ))
            })?
        };

        // SAFETY: lib.get returns a symbol from the loaded library.
        // Unsafe because there is no guarantee the symbol has the correct
        // type signature — the caller must ensure the plugin implements
        // the documented C ABI.
        let kind_fn: Symbol<KindFn> = unsafe {
            lib.get(b"syscity_driver_kind").map_err(|e| {
                crate::error::SyscityError::Plugin(format!(
                    "Missing 'syscity_driver_kind' in '{}': {}",
                    path.as_ref().display(),
                    e,
                ))
            })?
        };

        let create_fn: Symbol<CreateFn> = unsafe {
            lib.get(b"syscity_driver_create").map_err(|e| {
                crate::error::SyscityError::Plugin(format!(
                    "Missing 'syscity_driver_create' in '{}': {}",
                    path.as_ref().display(),
                    e,
                ))
            })?
        };

        let free_fn: Symbol<FreeFn> = unsafe {
            lib.get(b"syscity_driver_free").map_err(|e| {
                crate::error::SyscityError::Plugin(format!(
                    "Missing 'syscity_driver_free' in '{}': {}",
                    path.as_ref().display(),
                    e,
                ))
            })?
        };

        // SAFETY: Calling the function pointer from the loaded library.
        // The plugin guarantees it returns a valid C string.
        let kind_cstr = unsafe { CStr::from_ptr(kind_fn()) };
        let kind = kind_cstr
            .to_str()
            .map_err(|_| {
                crate::error::SyscityError::Plugin(format!(
                    "'syscity_driver_kind' returned invalid UTF-8 in '{}'",
                    path.as_ref().display(),
                ))
            })?
            .to_string();

        // The Symbol borrowing means we can't move the Library easily.
        // Detach the function pointers before dropping the Symbol refs.
        let create_fn_ptr = *create_fn;
        let free_fn_ptr = *free_fn;

        Ok(Self {
            _lib: lib,
            kind,
            create_fn: create_fn_ptr,
            free_fn: free_fn_ptr,
        })
    }

    /// The driver kind string reported by the plugin.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Convert this loader into a `DriverConstructor`.
    ///
    /// The returned constructor captures the create/free function pointers
    /// and can be registered in a `DriverFactory`.
    pub fn into_constructor(self) -> DriverConstructor {
        let create_fn = self.create_fn;
        let _free_fn = self.free_fn;
        let kind = self.kind.clone();

        Arc::new(move |params: Value| {
            // SAFETY: The caller ensures the plugin was loaded from a valid
            // library that exports the C ABI.  The opaque pointer returned
            // by `syscity_driver_create` is cast from `Box<dyn DeviceDriver>`.
            let params_str = params.to_string();
            let c_params = CString::new(params_str).map_err(|_| {
                crate::error::SyscityError::Plugin("Params contain null byte".into())
            })?;

            let ptr = unsafe { create_fn(c_params.as_ptr()) };
            if ptr.is_null() {
                return Err(crate::error::SyscityError::Plugin(format!(
                    "syscity_driver_create returned null for '{}'",
                    kind,
                )));
            }

            // SAFETY: The plugin allocated `Box<Box<dyn DeviceDriver>>`
            // and returned it as `*mut c_void`.  The double-Box is needed
            // because `Box<dyn DeviceDriver>` is a fat pointer and cannot
            // pass through the thin `*mut c_void` FFI boundary.
            let box_ptr: *mut Box<dyn DeviceDriver> = ptr as *mut Box<dyn DeviceDriver>;
            // SAFETY: box_ptr was created by the plugin via Box::into_raw
            // of a Box<Box<dyn DeviceDriver>>, matching this reconstruction.
            let inner: Box<dyn DeviceDriver> = *unsafe { Box::from_raw(box_ptr) };
            Ok(Arc::from(inner))
        })
    }
}

impl Drop for NativeDriverLoader {
    fn drop(&mut self) {
        // The `_lib: Library` is dropped here, unloading the shared library.
    }
}

// ── Directory scanning
// ────────────────────────────────────────────────────────

/// Scan a directory for native plugin shared libraries and load them.
///
/// Returns a list of `(kind, constructor)` tuples.  Failed loads are
/// logged as warnings and skipped.
pub fn scan_native_plugins(dir: &Path) -> Vec<(String, DriverConstructor)> {
    let mut plugins = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Failed to read native plugins directory {:?}: {}", dir, e);
            return plugins;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Check for shared library extensions
        let is_plugin = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| matches!(ext, "so" | "dylib" | "dll"));

        if !is_plugin || !path.is_file() {
            continue;
        }

        match NativeDriverLoader::load(&path) {
            Ok(loader) => {
                let kind = loader.kind().to_string();
                let ctor = loader.into_constructor();
                tracing::info!("Loaded native plugin: {} (kind: {})", path.display(), kind,);
                plugins.push((kind, ctor));
            }
            Err(e) => {
                tracing::warn!("Failed to load native plugin {:?}: {}", path, e);
            }
        }
    }

    plugins
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::*;

    // ── Test plugin helper ──────────────────────────────────────────────

    /// A minimal "plugin" compiled in the test binary itself.
    /// This simulates what a real .so/.dylib would export.
    mod test_plugin {
        use crate::device::driver::DeviceDriver;
        #[no_mangle]
        pub unsafe extern "C" fn syscity_driver_kind() -> *const std::ffi::c_char {
            b"test-native-driver\0".as_ptr() as *const std::ffi::c_char
        }

        #[no_mangle]
        pub unsafe extern "C" fn syscity_driver_create(
            _params: *const std::ffi::c_char,
        ) -> *mut std::ffi::c_void {
            // Double-Box: outer Box is thin, inner is the fat trait object.
            // This lets the pointer travel through C ABI as *mut c_void.
            let driver: Box<dyn DeviceDriver> =
                Box::new(crate::device::mock::MockDeviceDriver::new("native-device", true));
            let double_box: Box<Box<dyn DeviceDriver>> = Box::new(driver);
            Box::into_raw(double_box) as *mut std::ffi::c_void
        }

        #[no_mangle]
        pub unsafe extern "C" fn syscity_driver_free(ptr: *mut std::ffi::c_void) {
            if !ptr.is_null() {
                let box_ptr: *mut Box<dyn DeviceDriver> = ptr as *mut Box<dyn DeviceDriver>;
                let _ = Box::from_raw(box_ptr);
            }
        }
    }

    // ── Tests ───────────────────────────────────────────────────────────

    #[test]
    fn test_driver_kind_c_str() {
        let kind = unsafe { CStr::from_ptr(test_plugin::syscity_driver_kind()) };
        assert_eq!(kind.to_str().unwrap(), "test-native-driver");
    }

    #[test]
    fn test_driver_create_and_free() {
        let params = CString::new("{}").unwrap();
        let ptr = unsafe { test_plugin::syscity_driver_create(params.as_ptr()) };
        assert!(!ptr.is_null());

        // Recover the double-boxed driver
        let box_ptr: *mut Box<dyn DeviceDriver> = ptr as *mut Box<dyn DeviceDriver>;
        let inner: Box<dyn DeviceDriver> = *unsafe { Box::from_raw(box_ptr) };
        assert_eq!(inner.driver_name(), "native-device");
        // inner is dropped here
    }

    #[test]
    fn test_driver_free_null() {
        // Should not crash
        unsafe { test_plugin::syscity_driver_free(std::ptr::null_mut()) };
    }

    #[test]
    fn test_scan_nonexistent_dir() {
        let plugins = scan_native_plugins(Path::new("/nonexistent/plugin/dir"));
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_scan_empty_dir() {
        let dir = std::env::temp_dir().join(format!("syscity_test_plugins_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let plugins = scan_native_plugins(&dir);
        assert!(plugins.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_driver_create_with_empty_params() {
        let params = CString::new("{}").unwrap();
        let ptr = unsafe { test_plugin::syscity_driver_create(params.as_ptr()) };
        assert!(!ptr.is_null());
        unsafe { test_plugin::syscity_driver_free(ptr) };
    }
}
