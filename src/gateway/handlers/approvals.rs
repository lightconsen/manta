
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use std::sync::Arc;

use crate::gateway::GatewayState;
use crate::gateway::*;
use crate::tools::approval::{ApprovalDecision, ApprovalFilter};

// ── Tool approval management (human-in-the-loop) ──────────────────────────────

#[allow(dead_code)]
/// `GET /api/v1/approvals` — list all pending approval requests.
pub async fn list_approvals_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let approvals = state.tools.approval_queue
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
    if state.tools.approval_queue
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

    if state.tools.approval_queue
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
