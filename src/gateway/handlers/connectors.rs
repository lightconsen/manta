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

// ─────────────────────────────────────────────
// Marketplace catalog (P1-4 / P2-8)
// ─────────────────────────────────────────────

/// GET /api/v1/connectors/catalog — the marketplace catalog (cached) joined
/// with each entry's installed state.
///
/// On a first visit with an empty cache the handler one-shot syncs from the
/// cloud catalog URL when cloud mode is active (feature + `cloud.enabled` +
/// logged in), so member/cloud entries are visible immediately.
pub async fn catalog_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let manager = &state.tools.connector_manager;
    let doc: Option<crate::mcp::connectors::catalog::CatalogDocument> = {
        let cached = match manager.cached_catalog().await {
            Ok(Some(doc)) => Some(doc),
            Ok(None) => None,
            Err(e) => return Json(serde_json::json!({ "error": e.to_string() })),
        };
        #[cfg(feature = "cloud")]
        let synced = if cached.is_none() {
            let cfg = state.config.read().await.cloud.clone();
            if cfg.enabled && crate::cloud::session::logged_in().await {
                let url = format!("{}/catalog.json", cfg.api_base.trim_end_matches('/'));
                if let Ok((fresh, _)) = manager.sync_catalog(&url).await {
                    Some(fresh)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        #[cfg(not(feature = "cloud"))]
        let synced: Option<crate::mcp::connectors::catalog::CatalogDocument> = None;
        cached.or(synced)
    };

    let installed = manager.list().await.unwrap_or_default();
    let installed_by_id: std::collections::HashMap<String, _> =
        installed.into_iter().map(|s| (s.id.clone(), s)).collect();

    let entries: Vec<_> = doc
        .iter()
        .flat_map(|d| d.connectors.iter())
        .map(|e| {
            let inst = installed_by_id.get(&e.id);
            // Experts are "installed" once their role dir exists in agents/.
            let expert_installed =
                e.entry_type == "expert" && crate::dirs::agents_dir().join(&e.id).is_dir();
            serde_json::json!({
                "id": e.id,
                "version": e.version,
                "display_name": e.display_name,
                "description": e.description,
                "icon": e.icon,
                "type": e.entry_type,
                "kind": e.kind,
                "visibility": e.visibility,
                "credits_per_use": e.credits_per_use,
                "category": e.category,
                "installed": inst.is_some() || expert_installed,
                "installed_version": inst.map(|s| s.version.clone()),
                "state": inst.map(|s| serde_json::to_value(s.state).unwrap_or_default()),
            })
        })
        .collect();

    Json(serde_json::json!({
        "version": doc.as_ref().map(|d| d.version).unwrap_or(0),
        "synced": doc.is_some(),
        "entries": entries,
    }))
}

#[derive(Debug, Deserialize)]
pub struct CatalogInstallRequest {
    /// Connector id in the marketplace catalog.
    pub id: String,
}

/// POST /api/v1/connectors/catalog/install — install (or upgrade) a connector
/// from the cached marketplace catalog by id.
pub async fn catalog_install_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<CatalogInstallRequest>,
) -> impl IntoResponse {
    let manager = &state.tools.connector_manager;
    let Some(doc) = manager.cached_catalog().await.ok().flatten() else {
        return Json(
            serde_json::json!({ "error": "marketplace catalog is empty — sync it first" }),
        );
    };
    let Some(entry) = doc
        .connectors
        .into_iter()
        .filter(|e| e.id == body.id)
        .max_by_key(|e| catalog_version_key(&e.version))
    else {
        return Json(serde_json::json!({ "error": format!("no catalog entry for '{}'", body.id) }));
    };

    // Experts are role packages: install the role into agents/ and re-discover
    // so the expert becomes summonable (new session bound to its agent id).
    if entry.entry_type == "expert" {
        match manager.install_expert(&entry).await {
            Ok(agents) => {
                let _ = state.agents.registry.write().await.discover().await;
                Json(serde_json::json!({
                    "id": entry.id,
                    "type": "expert",
                    "agents": agents,
                    "installed": true,
                }))
            }
            Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
        }
    } else {
        match manager.upgrade(&entry).await {
            Ok(summary) => {
                let state_name = match summary.state {
                    crate::mcp::connectors::state::StateKind::Enabled => "enabled",
                    _ => "installed",
                };
                emit_connector_event(&state, &summary.id, state_name, Some(&summary)).await;
                Json(serde_json::json!({
                    "id": summary.id,
                    "version": summary.version,
                    "state": serde_json::to_value(summary.state).unwrap_or_default(),
                }))
            }
            Err(e) => Json(serde_json::json!({ "error": e.to_string() })),
        }
    }
}

/// Ordering key for catalog versions (semver when parseable, else raw string).
pub(crate) fn catalog_version_key(v: &str) -> (u64, u64, u64, String) {
    if let Ok(p) = crate::skills::semver::Version::parse(v) {
        (p.major, p.minor, p.patch, v.to_string())
    } else {
        (0, 0, 0, v.to_string())
    }
}
