//! WS admin handlers: audit log.

use std::sync::Arc;

use serde::Deserialize;

use super::super::{WsRequest, WsResponse};
use crate::gateway::GatewayState;

/// `audit.recent` — the most recent `{ limit }` audit entries, optionally
/// filtered by `{ event_type }`.
pub(crate) async fn handle_audit_recent(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        #[serde(default = "default_limit")]
        limit: usize,
        #[serde(default)]
        event_type: Option<String>,
    }
    fn default_limit() -> usize {
        50
    }
    let p: Params =
        match serde_json::from_value(req.params.clone().unwrap_or_else(|| serde_json::json!({}))) {
            Ok(p) => p,
            Err(e) => {
                return WsResponse::err(&req.id, "INVALID_PARAMS", format!("Invalid params: {}", e))
            }
        };
    let mut entries = state.auth.audit_log.recent(p.limit).await;
    if let Some(et) = p.event_type.as_deref() {
        entries.retain(|e| {
            format!("{:?}", e.event_type)
                .to_lowercase()
                .contains(&et.to_lowercase())
        });
    }
    WsResponse::ok(&req.id, serde_json::json!({ "entries": entries, "count": entries.len() }))
}

/// `audit.all` — all persisted audit entries (optionally `{ event_type }`).
pub(crate) async fn handle_audit_all(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        #[serde(default)]
        event_type: Option<String>,
    }
    let p: Params =
        match serde_json::from_value(req.params.clone().unwrap_or_else(|| serde_json::json!({}))) {
            Ok(p) => p,
            Err(e) => {
                return WsResponse::err(&req.id, "INVALID_PARAMS", format!("Invalid params: {}", e))
            }
        };
    let mut entries = state.auth.audit_log.all().await;
    if let Some(et) = p.event_type.as_deref() {
        entries.retain(|e| {
            format!("{:?}", e.event_type)
                .to_lowercase()
                .contains(&et.to_lowercase())
        });
    }
    WsResponse::ok(&req.id, serde_json::json!({ "entries": entries, "count": entries.len() }))
}
