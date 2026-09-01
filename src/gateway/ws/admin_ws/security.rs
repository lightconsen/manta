//! WS admin handlers: security — command gate, allowlist, and status.

use std::sync::Arc;

use serde::Deserialize;

use super::super::{parse_params, WsRequest, WsResponse};
use crate::gateway::GatewayState;

/// Parse a command-gate permission level from its CLI string.
fn parse_user_level(s: &str) -> Result<crate::tools::command_gate::UserLevel, String> {
    match s.to_lowercase().as_str() {
        "chat" => Ok(crate::tools::command_gate::UserLevel::Chat),
        "user" => Ok(crate::tools::command_gate::UserLevel::User),
        "admin" => Ok(crate::tools::command_gate::UserLevel::Admin),
        _ => Err(format!("Invalid gate level '{}': expected chat|user|admin", s)),
    }
}

/// `security.gate.set` — set a user's command-gate permission level
/// (`{ user_id, level: "chat" | "user" | "admin" }`).
pub(crate) async fn handle_security_gate_set(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        user_id: String,
        level: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let level = match parse_user_level(&p.level) {
        Ok(l) => l,
        Err(e) => return WsResponse::err(&req.id, "INVALID_PARAMS", e),
    };
    state.auth.command_gate.set_user_level(&p.user_id, level);
    WsResponse::ok(&req.id, serde_json::json!({ "user_id": p.user_id, "level": p.level }))
}

/// `security.gate.list` — all custom command-gate permission levels.
pub(crate) async fn handle_security_gate_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let levels = state.auth.command_gate.user_levels();
    let levels_json: serde_json::Map<String, serde_json::Value> = levels
        .into_iter()
        .map(|(user, level)| (user, serde_json::json!(level.to_string())))
        .collect();
    WsResponse::ok(&req.id, serde_json::json!({ "levels": levels_json }))
}

/// `security.gate.clear` — clear a user's custom gate level (`{ user_id }`).
pub(crate) async fn handle_security_gate_clear(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        user_id: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    state.auth.command_gate.clear_user_level(&p.user_id);
    WsResponse::ok(&req.id, serde_json::json!({ "user_id": p.user_id, "cleared": true }))
}

/// `security.allowlist.add` — add a user to the (optionally channel-scoped)
/// allowlist (`{ channel?, user_id, username? }`). Bypasses the pairing flow.
pub(crate) async fn handle_security_allowlist_add(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        #[serde(default)]
        channel: Option<String>,
        user_id: String,
        #[serde(default)]
        username: Option<String>,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let mut entry = match p.username {
        Some(ref u) => crate::security::allowlist::AllowlistEntry::by_username(&p.user_id, u),
        None => crate::security::allowlist::AllowlistEntry::by_id(&p.user_id, &p.user_id),
    };
    entry.channel_id = p.channel.as_ref().filter(|c| !c.is_empty()).cloned();
    state.auth.pairing_store.allowlist().add(entry).await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "user_id": p.user_id, "channel": p.channel, "added": true }),
    )
}

/// `security.allowlist.remove` — remove an allowlist entry by id
/// (`{ id }`).
pub(crate) async fn handle_security_allowlist_remove(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let removed = state.auth.pairing_store.allowlist().remove(&p.id).await;
    WsResponse::ok(&req.id, serde_json::json!({ "removed": removed }))
}

/// `security.allowlist.list` — list allowlist entries.
pub(crate) async fn handle_security_allowlist_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let entries = state.auth.pairing_store.allowlist().list().await;
    WsResponse::ok(&req.id, serde_json::json!({ "entries": entries, "count": entries.len() }))
}

/// `security.status` — a summary of the gateway's security posture.
pub(crate) async fn handle_security_status(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let config = state.config.read().await;
    let auth = &state.auth;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "auth_mode": format!("{:?}", config.security.auth_mode),
            "auth_required": config.security.auth_required,
            "shared_token_configured": config.security.shared_token.is_some(),
            "pairing_required": config.security.pairing_required,
            "gate_levels": auth.command_gate.user_levels().len(),
            "allowlist_count": auth.pairing_store.allowlist().len().await,
            "audit_log_count": auth.audit_log.persisted_count().await,
            "pending_pairings": auth.device_pairing_store.list_pending().await.len(),
            "authorized_devices": auth.device_pairing_store.list_authorized().await.len(),
        }),
    )
}
