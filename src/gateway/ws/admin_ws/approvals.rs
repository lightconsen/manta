//! WS admin handlers: approvals.

use std::sync::Arc;

use serde::Deserialize;

use super::super::{parse_params, WsRequest, WsResponse};
use crate::gateway::GatewayState;

// ── Approvals (human-in-the-loop tool approval) ─────────────────────────

/// `approvals.list` — pending tool-call approval requests.
pub(crate) async fn handle_approvals_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let approvals = state
        .tools
        .approval_queue
        .list_pending(crate::tools::approval::ApprovalFilter::default())
        .await;
    WsResponse::ok(&req.id, serde_json::json!({ "approvals": approvals, "count": approvals.len() }))
}

/// `approvals.get` — a single pending approval (`{ id }`).
pub(crate) async fn handle_approvals_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.tools.approval_queue.get(&id).await {
        Some(approval) => {
            WsResponse::ok(&req.id, serde_json::to_value(&approval).unwrap_or_default())
        }
        None => WsResponse::err(&req.id, "NOT_FOUND", &format!("Approval '{}' not found", id)),
    }
}

/// `approvals.approve` — approve a pending tool call (`{ id }`).
pub(crate) async fn handle_approvals_approve(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    if state
        .tools
        .approval_queue
        .resolve(&id, crate::tools::approval::ApprovalDecision::Approve)
        .await
    {
        WsResponse::ok(&req.id, serde_json::json!({ "id": id, "status": "approved" }))
    } else {
        WsResponse::err(&req.id, "NOT_FOUND", &format!("Approval '{}' not found", id))
    }
}

/// `approvals.deny` — deny a pending tool call (`{ id, reason? }`).
pub(crate) async fn handle_approvals_deny(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
        #[serde(default)]
        reason: Option<String>,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let reason = p.reason.unwrap_or_else(|| "Denied by operator".to_string());
    if state
        .tools
        .approval_queue
        .resolve(&p.id, crate::tools::approval::ApprovalDecision::Deny { reason: reason.clone() })
        .await
    {
        WsResponse::ok(
            &req.id,
            serde_json::json!({ "id": p.id, "status": "denied", "reason": reason }),
        )
    } else {
        WsResponse::err(&req.id, "NOT_FOUND", &format!("Approval '{}' not found", p.id))
    }
}
