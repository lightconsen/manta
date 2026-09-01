//! WS admin handlers: status.

use std::sync::Arc;

use super::super::{WsRequest, WsResponse};
use super::cloud_status_json;
use crate::gateway::GatewayState;

// ── Status ──────────────────────────────────────────────────────────────

/// `status.get` — engine status (agents, channels, version, cloud block).
pub(crate) async fn handle_status_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agents = state.agents.agents.read().await;
    let channels = state.channels.channels.read().await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "agents": {
                "total": agents.len(),
                "busy": agents.values().filter(|a| a.busy.load(std::sync::atomic::Ordering::Acquire)).count(),
            },
            "channels": channels.len(),
            "version": crate::VERSION,
            "cloud": cloud_status_json(state).await,
        }),
    )
}
