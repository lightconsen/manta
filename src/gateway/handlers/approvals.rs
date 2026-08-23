use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::gateway::GatewayState;
use crate::gateway::*;
use crate::tools::approval::{ApprovalDecision, ApprovalFilter};

// ── Tool approval management (human-in-the-loop)
// ──────────────────────────────

#[allow(dead_code)]
/// `GET /api/v1/approvals` — list all pending approval requests.
pub async fn list_approvals_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let approvals = state
        .tools
        .approval_queue
        .list_pending(ApprovalFilter::default())
        .await;
    Json(serde_json::json!({ "approvals": approvals, "count": approvals.len() }))
}

#[allow(dead_code)]
/// `GET /api/v1/approvals/:id` — get a specific pending approval.
pub async fn get_approval_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.tools.approval_queue.get(&id).await {
        Some(approval) => Json(approval).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Approval '{}' not found", id) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/approvals/:id/approve` — approve a pending tool call.
pub async fn approve_tool_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    if state
        .tools
        .approval_queue
        .resolve(&id, ApprovalDecision::Approve)
        .await
    {
        Json(serde_json::json!({ "id": id, "status": "approved" })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Approval '{}' not found", id) })),
        )
            .into_response()
    }
}

#[allow(dead_code)]
/// `POST /api/v1/approvals/:id/deny` — deny a pending tool call.
pub async fn deny_tool_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    body: Option<Json<DenyApprovalRequest>>,
) -> impl IntoResponse {
    let reason = body
        .and_then(|b| b.reason.clone())
        .unwrap_or_else(|| "Denied by operator".to_string());

    if state
        .tools
        .approval_queue
        .resolve(&id, ApprovalDecision::Deny { reason: reason.clone() })
        .await
    {
        Json(serde_json::json!({ "id": id, "status": "denied", "reason": reason })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Approval '{}' not found", id) })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;
    use crate::tools::approval::{ApprovalLevel, PendingApproval, RiskLevel};
    use tokio::sync::oneshot;

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    async fn state() -> Arc<GatewayState> {
        Arc::new(make_test_state(GatewayConfig::default()).await)
    }

    async fn submit_approval(state: &Arc<GatewayState>, id: &str) {
        let (tx, _rx) = oneshot::channel();
        let pa = PendingApproval::new(id, "bash", serde_json::json!({ "command": "ls" }), "alice")
            .with_risk_level(RiskLevel::High)
            .with_approval_level(ApprovalLevel::Ask)
            .with_message("Run bash")
            .with_response_tx(tx);
        state.tools.approval_queue.submit(pa).await;
    }

    #[tokio::test]
    async fn list_empty_state_returns_zero() {
        let state = state().await;
        let (status, json) =
            body_json(list_approvals_handler(State(state)).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["count"], 0);
        assert!(json["approvals"].is_array());
    }

    #[tokio::test]
    async fn list_with_pending_counts_and_contains_id() {
        let state = state().await;
        submit_approval(&state, "app-1").await;
        let (status, json) =
            body_json(list_approvals_handler(State(state)).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["count"], 1);
        assert_eq!(json["approvals"][0]["id"], "app-1");
    }

    #[tokio::test]
    async fn get_unknown_returns_404() {
        let state = state().await;
        let (status, json) = body_json(
            get_approval_handler(Path("missing".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(json["error"].as_str().unwrap().contains("missing"));
    }

    #[tokio::test]
    async fn get_found_returns_approval() {
        let state = state().await;
        submit_approval(&state, "app-2").await;
        let (status, json) = body_json(
            get_approval_handler(Path("app-2".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["id"], "app-2");
        assert_eq!(json["tool_name"], "bash");
    }

    #[tokio::test]
    async fn approve_unknown_returns_404() {
        let state = state().await;
        let (status, json) = body_json(
            approve_tool_handler(Path("missing".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(json["error"].as_str().unwrap().contains("missing"));
    }

    #[tokio::test]
    async fn approve_pending_returns_approved() {
        let state = state().await;
        submit_approval(&state, "app-3").await;
        let (status, json) = body_json(
            approve_tool_handler(Path("app-3".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["id"], "app-3");
        assert_eq!(json["status"], "approved");
        // Resolved approval is removed from the pending queue.
        let remaining = state
            .tools
            .approval_queue
            .list_pending(ApprovalFilter::default())
            .await;
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn deny_unknown_returns_404() {
        let state = state().await;
        let (status, json) = body_json(
            deny_tool_handler(Path("missing".into()), State(state), None)
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(json["error"].as_str().unwrap().contains("missing"));
    }

    #[tokio::test]
    async fn deny_pending_defaults_reason() {
        let state = state().await;
        submit_approval(&state, "app-4").await;
        let (status, json) = body_json(
            deny_tool_handler(Path("app-4".into()), State(state), None)
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["id"], "app-4");
        assert_eq!(json["status"], "denied");
        assert_eq!(json["reason"], "Denied by operator");
    }

    #[tokio::test]
    async fn deny_pending_with_reason_echoes_it() {
        let state = state().await;
        submit_approval(&state, "app-5").await;
        let body = Some(Json(DenyApprovalRequest {
            reason: Some("Not authorized".into()),
        }));
        let (status, json) = body_json(
            deny_tool_handler(Path("app-5".into()), State(state), body)
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["status"], "denied");
        assert_eq!(json["reason"], "Not authorized");
    }
}
