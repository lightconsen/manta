use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Json},
};
use serde::Deserialize;

use crate::gateway::ws::connectors_ws::emit_connector_event;
use crate::gateway::GatewayState;

// ─────────────────────────────────────────────
// Connector REST API handlers (P0-1)
// ─────────────────────────────────────────────

/// List installed connectors with their lifecycle state.
pub async fn list_connectors_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    match state.tools.connector_manager.list().await {
        Ok(connectors) => {
            let entries: Vec<_> = connectors
                .iter()
                .map(|s| serde_json::to_value(s).unwrap_or_default())
                .collect();
            Json(serde_json::json!({ "connectors": entries, "count": entries.len() }))
        }
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

#[derive(Debug, Deserialize)]
pub struct InstallConnectorRequest {
    pub source_dir: String,
}

/// Install a connector from a local package directory (`connector.json`).
pub async fn install_connector_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<InstallConnectorRequest>,
) -> impl IntoResponse {
    match state
        .tools
        .connector_manager
        .install_from_dir(std::path::Path::new(&body.source_dir))
        .await
    {
        Ok(summary) => {
            let state_name = match summary.state {
                crate::mcp::connectors::state::StateKind::Enabled => "enabled",
                _ => "installed",
            };
            emit_connector_event(&state, &summary.id, state_name, Some(&summary)).await;
            Json(serde_json::to_value(&summary).unwrap_or_default())
        }
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// Enable a connector (connects the underlying MCP server, if any).
pub async fn enable_connector_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.tools.connector_manager.enable(&id).await {
        Ok(summary) => {
            emit_connector_event(&state, &id, "enabled", Some(&summary)).await;
            Json(serde_json::to_value(&summary).unwrap_or_default())
        }
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// Disable a connector.
pub async fn disable_connector_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.tools.connector_manager.disable(&id).await {
        Ok(summary) => {
            emit_connector_event(&state, &id, "disabled", Some(&summary)).await;
            Json(serde_json::to_value(&summary).unwrap_or_default())
        }
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// Uninstall a connector (refused while enabled).
pub async fn uninstall_connector_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.tools.connector_manager.uninstall(&id).await {
        Ok(()) => {
            emit_connector_event(&state, &id, "uninstalled", None).await;
            Json(serde_json::json!({ "id": id, "state": "uninstalled" }))
        }
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// Report a connector's auth status.
pub async fn connector_auth_status_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.tools.connector_manager.auth_status(&id).await {
        Ok(text) => Json(serde_json::json!({
            "id": id,
            "authenticated": text.is_some(),
            "text": text,
        })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

#[derive(Debug, Deserialize)]
pub struct SyncCatalogRequest {
    pub url: String,
}

/// Sync the marketplace catalog from `url` (ETag/304-aware).
pub async fn sync_connectors_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SyncCatalogRequest>,
) -> impl IntoResponse {
    match state.tools.connector_manager.sync_catalog(&body.url).await {
        Ok((doc, refreshed)) => Json(serde_json::json!({
            "refreshed": refreshed,
            "version": doc.version,
            "entries": doc.connectors.len(),
        })),
        Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
    }
}

/// Report pending connector updates, or apply them when `apply=true`.
pub async fn connector_updates_handler(
    State(state): State<Arc<GatewayState>>,
    axum::extract::Query(params): axum::extract::Query<UpdatesQuery>,
) -> impl IntoResponse {
    if params.apply {
        match state
            .tools
            .connector_manager
            .apply_updates(params.auto_only)
            .await
        {
            Ok(applied) => {
                for id in &applied {
                    emit_connector_event(&state, id, "updated", None).await;
                }
                Json(serde_json::json!({ "applied": applied }))
            }
            Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
        }
    } else {
        match state.tools.connector_manager.check_updates().await {
            Ok(pending) => {
                let entries: Vec<_> = pending
                    .iter()
                    .map(|u| {
                        serde_json::json!({
                            "id": u.id,
                            "current_version": u.current_version,
                            "latest_version": u.latest_version,
                            "auto_update": u.entry.auto_update,
                        })
                    })
                    .collect();
                Json(serde_json::json!({ "pending": entries }))
            }
            Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdatesQuery {
    #[serde(default)]
    pub auto_only: bool,
    #[serde(default)]
    pub apply: bool,
}
