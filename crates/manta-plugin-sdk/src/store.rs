//! Persistent key-value store
//!
//! Data persists across plugin reloads. Scoped by plugin ID — plugins cannot
//! access each other's store entries.

use crate::ffi_call_to_string;
use std::string::String;

/// Set a key-value pair in the persistent store. Returns `true` on success.
///
/// # Example
/// ```ignore
/// store::set("user:123:preferences", r#"{"theme":"dark"}"#);
/// ```
pub fn set(key: &str, value: &str) -> bool {
    unsafe { super::store_set(key.as_ptr(), key.len(), value.as_ptr(), value.len()) > 0 }
}

/// Get a value from the persistent store. Returns `None` if the key does not exist.
///
/// # Example
/// ```ignore
/// if let Some(prefs) = store::get("user:123:preferences") {
///     let prefs: serde_json::Value = serde_json::from_str(&prefs).unwrap();
/// }
/// ```
pub fn get(key: &str) -> Option<String> {
    ffi_call_to_string(|out_ptr, out_len| unsafe {
        super::store_get(key.as_ptr(), key.len(), out_ptr, out_len)
    })
}

/// Delete a key from the persistent store (by setting it to empty).
pub fn delete(key: &str) -> bool {
    set(key, "")
}
