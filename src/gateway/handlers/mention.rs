use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::gateway::GatewayState;
use crate::gateway::*;

#[allow(dead_code)]
/// `GET /api/v1/mentions/policy` — get current mention gate policy.
pub async fn get_mention_policy_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let policy = state.auth.mention_gate.policy().await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "policy": policy.to_string(),
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `POST /api/v1/mentions/policy` — set mention gate policy.
pub async fn set_mention_policy_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SetMentionPolicyRequest>,
) -> impl IntoResponse {
    state.auth.mention_gate.set_policy(req.policy).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok",
            "policy": req.policy.to_string(),
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `GET /api/v1/mentions/allowlist` — list allowlist entries for a channel.
pub async fn list_mention_allowlist_handler(
    State(state): State<Arc<GatewayState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let channel = params
        .get("channel")
        .cloned()
        .unwrap_or_else(|| "*".to_string());
    let entries = state.auth.mention_gate.list_allowlist(&channel).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "channel": channel,
            "allowlist": entries,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `POST /api/v1/mentions/allowlist` — add a pattern to the allowlist.
pub async fn add_mention_allowlist_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AddMentionPatternRequest>,
) -> impl IntoResponse {
    state
        .auth
        .mention_gate
        .add_allowlist(&req.channel, &req.pattern)
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "added",
            "channel": req.channel,
            "pattern": req.pattern,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `DELETE /api/v1/mentions/allowlist/:channel/:pattern` — remove from
/// allowlist.
pub async fn remove_mention_allowlist_handler(
    State(state): State<Arc<GatewayState>>,
    Path((channel, pattern)): Path<(String, String)>,
) -> impl IntoResponse {
    let removed = state
        .auth
        .mention_gate
        .remove_allowlist(&channel, &pattern)
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": if removed { "removed" } else { "not_found" },
            "channel": channel,
            "pattern": pattern,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `GET /api/v1/mentions/blocklist` — list blocklist entries for a channel.
pub async fn list_mention_blocklist_handler(
    State(state): State<Arc<GatewayState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let channel = params
        .get("channel")
        .cloned()
        .unwrap_or_else(|| "*".to_string());
    let entries = state.auth.mention_gate.list_blocklist(&channel).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "channel": channel,
            "blocklist": entries,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `POST /api/v1/mentions/blocklist` — add a pattern to the blocklist.
pub async fn add_mention_blocklist_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AddMentionPatternRequest>,
) -> impl IntoResponse {
    state
        .auth
        .mention_gate
        .add_blocklist(&req.channel, &req.pattern)
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "added",
            "channel": req.channel,
            "pattern": req.pattern,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `DELETE /api/v1/mentions/blocklist/:channel/:pattern` — remove from
/// blocklist.
pub async fn remove_mention_blocklist_handler(
    State(state): State<Arc<GatewayState>>,
    Path((channel, pattern)): Path<(String, String)>,
) -> impl IntoResponse {
    let removed = state
        .auth
        .mention_gate
        .remove_blocklist(&channel, &pattern)
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": if removed { "removed" } else { "not_found" },
            "channel": channel,
            "pattern": pattern,
        })),
    )
        .into_response()
}
