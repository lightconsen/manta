#![allow(unused_imports)]

use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::{Agent, AgentConfig};
use crate::canvas::{CanvasEvent, CanvasManager};
use crate::channels::{Channel, ChannelExtension, ChannelType};
use crate::config::hot_reload::{ConfigFileType, HotReloadManager};
use crate::gateway::GatewayState;
use crate::gateway::*;
use crate::inbound::*;
use crate::memory::vector::{
    ApiEmbeddingProvider, CachedEmbeddingProvider, EmbeddingConfig, LocalGgufEmbeddingProvider,
    MemoryVectorStore, VectorMemoryService,
};
use crate::model_router::ModelRouter;
use crate::plugins::PluginManager;
use crate::security::pairing::DmPolicy;
use crate::tools::approval::{ApprovalDecision, ApprovalFilter, ApprovalQueue};
use crate::tools::mcp::{McpManager, McpSettings, McpToolWrapper};
use crate::tools::ToolRegistry;

// Plugin Management API Handlers

// ── Request / Query types ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct InstallPluginRequest {
    pub name: String,
    pub registry: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SignPluginRequest {
    pub name: String,
    pub secret_key: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub registry: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────

#[allow(dead_code)]
pub async fn list_plugins_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let plugins = state.infra.plugin_manager.list_plugins().await;
    let plugin_list: Vec<_> = plugins
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id(),
                "name": p.name(),
                "enabled": p.enabled,
                "capabilities": p.manifest.capabilities,
            })
        })
        .collect();

    Json(serde_json::json!({
        "plugins": plugin_list,
        "count": plugin_list.len(),
    }))
}

#[allow(dead_code)]
pub async fn enable_plugin_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.plugin_manager.set_enabled(&id, true).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Plugin '{}' enabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to enable plugin: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn disable_plugin_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.plugin_manager.set_enabled(&id, false).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Plugin '{}' disabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to disable plugin: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn unload_plugin_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.plugin_manager.unload_plugin(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => {
            let error = serde_json::json!({
                "error": format!("Plugin '{}' not found", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to unload plugin: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn reload_plugin_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.plugin_manager.reload_plugin(&id).await {
        Ok(reloaded_id) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Plugin '{}' reloaded", reloaded_id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to reload plugin: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn reload_plugins_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    // Unload all currently loaded plugins, then re-initialize from disk.
    let plugins = state.infra.plugin_manager.list_plugins().await;
    let ids: Vec<String> = plugins.iter().map(|p| p.id().to_string()).collect();
    let mut unloaded = 0usize;
    for id in &ids {
        match state.infra.plugin_manager.unload_plugin(id).await {
            Ok(_) => unloaded += 1,
            Err(e) => warn!("Failed to unload plugin '{}' during reload: {}", id, e),
        }
    }
    match state.infra.plugin_manager.initialize().await {
        Ok(loaded) => {
            let response = serde_json::json!({
                "success": true,
                "unloaded": unloaded,
                "loaded": loaded,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Reload failed: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

/// POST /api/v1/plugins/install — Install a plugin from a remote registry.
#[allow(dead_code)]
pub async fn install_plugin_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<InstallPluginRequest>,
) -> impl IntoResponse {
    match state.infra.plugin_manager
        .install_plugin(&req.name, req.registry.as_deref())
        .await
    {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Plugin '{}' installed", req.name),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to install plugin: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

/// POST /api/v1/plugins/uninstall — Uninstall a plugin (remove from disk).
#[allow(dead_code)]
pub async fn uninstall_plugin_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<InstallPluginRequest>,
) -> impl IntoResponse {
    match state.infra.plugin_manager.uninstall_plugin(&req.name).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Plugin '{}' uninstalled", req.name),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to uninstall plugin: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

/// GET /api/v1/plugins/search?q=...&registry=... — Search for plugins.
#[allow(dead_code)]
pub async fn search_plugins_handler(
    State(state): State<Arc<GatewayState>>,
    Query(params): Query<SearchQuery>,
) -> impl IntoResponse {
    match state.infra.plugin_manager
        .search_registry(&params.q, params.registry.as_deref())
        .await
    {
        Ok(results) => {
            let results_json: Vec<serde_json::Value> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "name": r.name,
                        "version": r.version,
                        "description": r.description,
                        "author": r.author,
                    })
                })
                .collect();
            (StatusCode::OK, Json(serde_json::json!({ "results": results_json }))).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to search registry: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

/// POST /api/v1/plugins/sign — Sign a plugin manifest with an ed25519 key.
#[allow(dead_code)]
pub async fn sign_plugin_handler(
    State(_state): State<Arc<GatewayState>>,
    Json(req): Json<SignPluginRequest>,
) -> impl IntoResponse {
    use crate::plugins::manifest::PluginManifest;
    use crate::plugins::verification::sign_manifest;

    // Find the plugin directory
    let plugin_dir = crate::dirs::config_dir().join("plugins").join(&req.name);
    let manifest_path = plugin_dir.join("plugin.json");
    if !manifest_path.exists() {
        let error = serde_json::json!({
            "error": format!("Plugin '{}' not found at {:?}", req.name, manifest_path),
        });
        return (StatusCode::NOT_FOUND, Json(error)).into_response();
    }

    // Read and parse manifest
    let content = match tokio::fs::read_to_string(&manifest_path).await {
        Ok(c) => c,
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to read manifest: {}", e),
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response();
        }
    };
    let mut manifest: PluginManifest = match serde_json::from_str(&content) {
        Ok(m) => m,
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to parse manifest: {}", e),
            });
            return (StatusCode::BAD_REQUEST, Json(error)).into_response();
        }
    };

    // Sign
    if let Err(e) = sign_manifest(&mut manifest, &req.secret_key) {
        let error = serde_json::json!({
            "error": format!("Failed to sign manifest: {}", e),
        });
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response();
    }

    // Write back
    match tokio::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    )
    .await
    {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Plugin '{}' signed successfully", req.name),
                "signer_public_key": manifest.signer_public_key,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to write signed manifest: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}
