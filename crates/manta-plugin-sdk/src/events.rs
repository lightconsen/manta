//! Event emission
//!
//! Plugins can emit events that Manta consumers (hooks, other plugins,
//! external systems) can subscribe to.

/// Emit an event with a type name and a JSON payload.
///
/// Returns `true` if the event was sent, `false` if no event channel is configured.
///
/// # Example
/// ```ignore
/// events::emit(
///     "user.created",
///     &serde_json::json!({
///         "user_id": "123",
///         "name": "Alice"
///     }),
/// );
/// ```
pub fn emit(event_type: &str, payload: &serde_json::Value) -> bool {
    let payload_str = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    unsafe {
        super::emit_event(
            event_type.as_ptr(),
            event_type.len(),
            payload_str.as_ptr(),
            payload_str.len(),
        ) > 0
    }
}
