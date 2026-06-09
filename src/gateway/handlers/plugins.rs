#![allow(unused_imports)]

use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
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
use crate::gateway::GatewayState;
use crate::gateway::*;

// Plugin Management API Handlers

#[allow(dead_code)]
pub async fn list_plugins_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let plugins = state.plugin_manager.list_plugins().await;
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
    match state.plugin_manager.set_enabled(&id, true).await {
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
    match state.plugin_manager.set_enabled(&id, false).await {
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
    match state.plugin_manager.unload_plugin(&id).await {
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
    match state.plugin_manager.reload_plugin(&id).await {
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
    let plugins = state.plugin_manager.list_plugins().await;
    let ids: Vec<String> = plugins.iter().map(|p| p.id().to_string()).collect();
    let mut unloaded = 0usize;
    for id in &ids {
        match state.plugin_manager.unload_plugin(id).await {
            Ok(_) => unloaded += 1,
            Err(e) => warn!("Failed to unload plugin '{}' during reload: {}", id, e),
        }
    }
    match state.plugin_manager.initialize().await {
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

