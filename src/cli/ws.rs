//! CLI WebSocket helper — calls the gateway's WS RPC surface.
//!
//! The CLI is migrating from REST (`DAEMON_URL/api/v1/...`) to the same WS
//! transport the UI uses. `call` opens a short-lived connection, performs the
//! `connect` handshake, sends one method, and returns the payload.

use serde_json::Value;

use crate::client::DaemonClient;
use crate::error::SyscityError;

/// The local daemon endpoint the CLI talks to.
pub const DAEMON_HOST: &str = "127.0.0.1";
pub const DAEMON_PORT: u16 = 18080;

/// Invoke one WS method on the daemon and return the response payload.
pub async fn call(method: &str, params: Value) -> crate::Result<Value> {
    let client = DaemonClient::with_ws(DAEMON_HOST, DAEMON_PORT);
    client.ws_call(method, params).await
}

/// Convenience: invoke a WS method and parse the payload into `T`.
#[allow(dead_code)]
pub async fn call_typed<T: serde::de::DeserializeOwned>(
    method: &str,
    params: Value,
) -> crate::Result<T> {
    let payload = call(method, params).await?;
    serde_json::from_value(payload)
        .map_err(|e| SyscityError::Internal(format!("Invalid WS response: {}", e)))
}
