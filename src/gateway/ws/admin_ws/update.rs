//! WS admin handlers: update.

use std::sync::Arc;

use super::super::{WsRequest, WsResponse};
use crate::gateway::GatewayState;

// ── Update ──────────────────────────────────────────────────────────────

/// `update.status` — current/latest release info.
pub(crate) async fn handle_update_status(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    if let Some(cache) = state.update.status_cache.read().await.as_ref() {
        WsResponse::ok(&req.id, serde_json::to_value(&cache.info).unwrap_or_default())
    } else {
        WsResponse::ok(&req.id, serde_json::json!({ "enabled": false }))
    }
}

/// `update.progress` — current update phase/percent (polled).
pub(crate) async fn handle_update_progress(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let progress = state.update.progress.read().await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "phase": progress.phase,
            "percent": progress.percent,
            "error": progress.error,
            "current": progress.current,
            "latest": progress.latest,
        }),
    )
}

/// `update.trigger` — start the self-update flow (same checks as
/// `POST /api/v1/update`). Rejected when embedded (desktop uses the Tauri
/// updater instead) or disabled in config.
pub(crate) async fn handle_update_trigger(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    use crate::gateway::handlers::update::{run_update_task, set_progress};
    use crate::gateway::state::{UpdatePhase, UpdateProgress};

    if state.embedded {
        return WsResponse::err(
            &req.id,
            "CONFLICT",
            "This syscity instance is embedded in the desktop app; use the desktop updater instead.",
        );
    }
    if !state.config.read().await.update.enabled {
        return WsResponse::err(
            &req.id,
            "FORBIDDEN",
            "Online updates are disabled in the configuration.",
        );
    }
    {
        let progress = state.update.progress.read().await;
        let busy = matches!(
            progress.phase,
            UpdatePhase::Checking
                | UpdatePhase::Downloading
                | UpdatePhase::Verifying
                | UpdatePhase::Applying
                | UpdatePhase::Restarting
        );
        if busy {
            return WsResponse::err(&req.id, "CONFLICT", "An update is already in progress.");
        }
    }

    *state.update.progress.write().await = UpdateProgress::idle(crate::VERSION);
    set_progress(state, UpdatePhase::Checking, 5, None).await;

    let host = state.config.read().await.host.clone();
    let port = state.config.read().await.port;
    let task_state = state.clone();
    let shutdown_token = state.shutdown_token.clone();
    let handle = tokio::spawn(async move {
        run_update_task(task_state, shutdown_token, host, port).await;
    });
    state
        .task_registry
        .insert_join("update:apply", handle)
        .await;

    WsResponse::ok(&req.id, serde_json::json!({ "status": "started" }))
}
