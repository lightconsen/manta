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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;
    use crate::security::mention_gate::MentionPolicy;

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn policy_get_and_set_roundtrip() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            get_mention_policy_handler(State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["policy"].as_str().is_some());

        let (status, body) = body_json(
            set_mention_policy_handler(
                State(state.clone()),
                Json(SetMentionPolicyRequest { policy: MentionPolicy::Block }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["policy"].as_str(), Some("block"));
        assert_eq!(state.auth.mention_gate.policy().await, MentionPolicy::Block);
    }

    #[tokio::test]
    async fn allowlist_add_list_remove() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            add_mention_allowlist_handler(
                State(state.clone()),
                Json(AddMentionPatternRequest {
                    channel: "telegram".into(),
                    pattern: "bob".into(),
                }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"].as_str(), Some("added"));

        let (_, body) = body_json(
            list_mention_allowlist_handler(
                State(state.clone()),
                axum::extract::Query(std::collections::HashMap::from([(
                    "channel".to_string(),
                    "telegram".to_string(),
                )])),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(body["channel"].as_str(), Some("telegram"));
        assert_eq!(body["allowlist"].as_array().map(|a| a.len()), Some(1));

        let (status, body) = body_json(
            remove_mention_allowlist_handler(
                State(state.clone()),
                Path(("telegram".to_string(), "bob".to_string())),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"].as_str(), Some("removed"));

        // Removing again reports not_found.
        let (_, body) = body_json(
            remove_mention_allowlist_handler(
                State(state),
                Path(("telegram".to_string(), "bob".to_string())),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(body["status"].as_str(), Some("not_found"));
    }

    #[tokio::test]
    async fn blocklist_add_list_remove() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            add_mention_blocklist_handler(
                State(state.clone()),
                Json(AddMentionPatternRequest {
                    channel: "discord".into(),
                    pattern: "spammer".into(),
                }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (_, body) = body_json(
            list_mention_blocklist_handler(
                State(state.clone()),
                axum::extract::Query(std::collections::HashMap::new()),
            )
            .await
            .into_response(),
        )
        .await;
        // Default channel is "*"; entry was on "discord" → not visible here.
        assert_eq!(body["channel"].as_str(), Some("*"));

        let (_, body) = body_json(
            list_mention_blocklist_handler(
                State(state.clone()),
                axum::extract::Query(std::collections::HashMap::from([(
                    "channel".to_string(),
                    "discord".to_string(),
                )])),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(body["blocklist"].as_array().map(|a| a.len()), Some(1));

        let (_, body) = body_json(
            remove_mention_blocklist_handler(
                State(state),
                Path(("discord".to_string(), "spammer".to_string())),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(body["status"].as_str(), Some("removed"));
    }
}
