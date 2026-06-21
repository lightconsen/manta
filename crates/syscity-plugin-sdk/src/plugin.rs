//! Plugin identity
//!
//! Access the plugin's own identity information.

use std::string::String;

use crate::ffi_call_to_string;

/// Get this plugin's unique identifier.
///
/// # Example
/// ```ignore
/// let id = plugin::id();
/// logging::info(&format!("Running plugin: {}", id));
/// ```
pub fn id() -> String {
    ffi_call_to_string(|out_ptr, out_len| unsafe { super::get_plugin_id(out_ptr, out_len) })
        .unwrap_or_default()
}
