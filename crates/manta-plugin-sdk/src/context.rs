//! Manta context access
//!
//! Read session ID and arbitrary context values set by Manta.

use crate::ffi_call_to_string;
use std::string::String;

/// Get the current session ID. Returns `None` if no session is active.
///
/// # Example
/// ```ignore
/// if let Some(session) = context::session_id() {
///     logging::info(&format!("Running in session {}", session));
/// }
/// ```
pub fn session_id() -> Option<String> {
    ffi_call_to_string(|out_ptr, out_len| unsafe { super::get_session_id(out_ptr, out_len) })
}

/// Get a context value by key. Returns `None` if the key does not exist.
///
/// # Example
/// ```ignore
/// if let Some(channel) = context::get("channel") {
///     // adapt output to the channel
/// }
/// ```
pub fn get(key: &str) -> Option<String> {
    ffi_call_to_string(|out_ptr, out_len| unsafe {
        super::get_context(key.as_ptr(), key.len(), out_ptr, out_len)
    })
}
