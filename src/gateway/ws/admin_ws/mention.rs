//! WS admin handlers: mention.

use std::sync::Arc;

use super::super::{parse_params, WsRequest, WsResponse};
use crate::gateway::GatewayState;

// ── Mention gate policy / allowlist / blocklist ─────────────────────────

/// `mention.policy` — current mention gate policy.
pub(crate) async fn handle_mention_policy_get(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let policy = state.auth.mention_gate.policy().await;
    WsResponse::ok(&req.id, serde_json::json!({ "policy": policy.to_string() }))
}

/// `mention.policy.set` — `{ policy }`.
pub(crate) async fn handle_mention_policy_set(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::SetMentionPolicyRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    state.auth.mention_gate.set_policy(body.policy).await;
    let policy = state.auth.mention_gate.policy().await;
    WsResponse::ok(&req.id, serde_json::json!({ "status": "ok", "policy": policy.to_string() }))
}

/// `mention.allowlist` — `{ channel? }` (default "*") list allowlist entries.
pub(crate) async fn handle_mention_allowlist_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let channel = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["channel"].as_str().unwrap_or("*").to_string(),
        Err(res) => return res,
    };
    let entries = state.auth.mention_gate.list_allowlist(&channel).await;
    WsResponse::ok(&req.id, serde_json::json!({ "channel": channel, "allowlist": entries }))
}

/// `mention.allowlist.add` — `{ channel, pattern }`.
pub(crate) async fn handle_mention_allowlist_add(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::AddMentionPatternRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    state
        .auth
        .mention_gate
        .add_allowlist(&body.channel, &body.pattern)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "status": "added", "channel": body.channel, "pattern": body.pattern }),
    )
}

/// `mention.allowlist.remove` — `{ channel, pattern }`.
pub(crate) async fn handle_mention_allowlist_remove(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::AddMentionPatternRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let removed = state
        .auth
        .mention_gate
        .remove_allowlist(&body.channel, &body.pattern)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": if removed { "removed" } else { "not_found" },
            "channel": body.channel,
            "pattern": body.pattern,
        }),
    )
}

/// `mention.blocklist` — `{ channel? }` (default "*") list blocklist entries.
pub(crate) async fn handle_mention_blocklist_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let channel = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["channel"].as_str().unwrap_or("*").to_string(),
        Err(res) => return res,
    };
    let entries = state.auth.mention_gate.list_blocklist(&channel).await;
    WsResponse::ok(&req.id, serde_json::json!({ "channel": channel, "blocklist": entries }))
}

/// `mention.blocklist.add` — `{ channel, pattern }`.
pub(crate) async fn handle_mention_blocklist_add(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::AddMentionPatternRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    state
        .auth
        .mention_gate
        .add_blocklist(&body.channel, &body.pattern)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "status": "added", "channel": body.channel, "pattern": body.pattern }),
    )
}

/// `mention.blocklist.remove` — `{ channel, pattern }`.
pub(crate) async fn handle_mention_blocklist_remove(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::AddMentionPatternRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let removed = state
        .auth
        .mention_gate
        .remove_blocklist(&body.channel, &body.pattern)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": if removed { "removed" } else { "not_found" },
            "channel": body.channel,
            "pattern": body.pattern,
        }),
    )
}
