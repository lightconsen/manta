//! In-memory key-value store
//!
//! Per-plugin, in-memory storage. Data is lost when the plugin is reloaded.
//! Use [`store`](super::store) for persistent storage.

use std::string::{String, ToString};
use std::vec::Vec;
use crate::ffi_call_to_string;

/// Store a value in memory. Returns `true` on success.
///
/// # Example
/// ```ignore
/// memory::store("temp_data", b"hello world");
/// ```
pub fn store(key: &str, value: &[u8]) -> bool {
    unsafe {
        super::memory_store(key.as_ptr(), key.len(), value.as_ptr(), value.len()) > 0
    }
}

/// Store a string value in memory.
///
/// # Example
/// ```ignore
/// memory::store_str("counter", "42");
/// ```
pub fn store_str(key: &str, value: &str) -> bool {
    store(key, value.as_bytes())
}

/// Load a value from memory. Returns `None` if the key does not exist.
///
/// # Example
/// ```ignore
/// if let Some(data) = memory::load("temp_data") {
///     // use data
/// }
/// ```
pub fn load(key: &str) -> Option<Vec<u8>> {
    ffi_call_to_string(|out_ptr, out_len| unsafe {
        super::memory_load(key.as_ptr(), key.len(), out_ptr, out_len)
    })
    .map(|s| s.into_bytes())
}

/// Load a string value from memory.
pub fn load_str(key: &str) -> Option<String> {
    ffi_call_to_string(|out_ptr, out_len| unsafe {
        super::memory_load(key.as_ptr(), key.len(), out_ptr, out_len)
    })
}

/// Search for keys matching a prefix. Returns a JSON array of key names.
///
/// # Example
/// ```ignore
/// let keys_json = memory::search("cache:");
/// let keys: Vec<String> = serde_json::from_str(&keys_json).unwrap();
/// ```
pub fn search(prefix: &str) -> String {
    ffi_call_to_string(|out_ptr, out_len| unsafe {
        super::memory_search(prefix.as_ptr(), prefix.len(), out_ptr, out_len)
    })
    .unwrap_or_else(|| "[]".to_string())
}

/// Write a JSON value to the result buffer (for use in tool implementations).
///
/// # Example
/// ```ignore
/// let result = serde_json::json!({ "status": "ok" });
/// unsafe {
///     memory::write_result(out_ptr, out_max, &result);
/// }
/// ```
///
/// # Safety
/// `out_ptr` must be a valid writable pointer into WASM linear memory,
/// and `out_max` must not exceed the buffer size.
pub unsafe fn write_result(out_ptr: *mut u8, out_max: usize, result: &serde_json::Value) -> usize {
    let json = serde_json::to_string(result).unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.to_string());
    let bytes = json.as_bytes();
    let to_write = bytes.len().min(out_max);
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_ptr, to_write);
    }
    to_write
}

/// Read a string from WASM memory (useful for reading tool parameters).
///
/// # Safety
/// `ptr` and `len` must point to valid readable WASM linear memory
/// and the region must contain valid UTF-8.
pub unsafe fn read_string(ptr: *const u8, len: usize) -> String {
    unsafe {
        let slice = core::slice::from_raw_parts(ptr, len);
        core::str::from_utf8(slice)
            .map(|s| s.to_string())
            .unwrap_or_default()
    }
}
