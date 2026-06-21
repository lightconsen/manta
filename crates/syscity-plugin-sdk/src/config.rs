//! Plugin configuration
//!
//! Access plugin configuration values set in `plugin.json` / `config.json`.

use std::string::String;

use crate::ffi_call_to_string;

/// Get a config value by key. Returns `None` if the key does not exist.
///
/// # Example
/// ```ignore
/// if let Some(api_key) = config::get("api_key") {
///     // use the key
/// }
/// ```
pub fn get(key: &str) -> Option<String> {
    ffi_call_to_string(|out_ptr, out_len| unsafe {
        super::config_get(key.as_ptr(), key.len(), out_ptr, out_len)
    })
}

/// Get the entire config as a JSON string.
///
/// # Example
/// ```ignore
/// let config_json = config::get_all();
/// let config: serde_json::Value = serde_json::from_str(&config_json).unwrap();
/// ```
pub fn get_all() -> String {
    ffi_call_to_string(|out_ptr, out_len| unsafe { super::config_get_all(out_ptr, out_len) })
        .unwrap_or_else(|| "{}".to_string())
}
