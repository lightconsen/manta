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

// Provider Management Handlers

#[allow(dead_code)]
pub async fn list_providers_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let providers = state.model_router.list_providers().await;
    Json(serde_json::json!({
        "providers": providers,
        "count": providers.len(),
    }))
}

#[allow(dead_code)]
pub async fn get_provider_health_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.get_provider_health(&id).await {
        Some(health) => {
            let response = serde_json::json!({
                "provider": id,
                "health": health,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": format!("Provider '{}' not found", id),
                "provider": id,
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn switch_model_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SwitchModelRequest>,
) -> impl IntoResponse {
    match state.model_router.switch_default_model(&body.model).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Switched to model '{}'", body.model),
                "current_model": body.model,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn enable_provider_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.enable_provider(&id).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Provider '{}' enabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn disable_provider_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.disable_provider(&id).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Provider '{}' disabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn check_provider_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.check_provider_health(&id).await {
        Ok(healthy) => {
            let response = serde_json::json!({
                "provider": id,
                "healthy": healthy,
                "checked_at": chrono::Utc::now().to_rfc3339(),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

// Provider Usage Handlers

#[allow(dead_code)]
pub async fn provider_usage_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let snapshots = state.model_router.all_snapshots_with_quota().await;
    Json(serde_json::json!({
        "providers": snapshots,
        "count": snapshots.len(),
    }))
}

#[allow(dead_code)]
pub async fn provider_usage_by_id_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.snapshot_with_quota(&id).await {
        Some(snapshot) => (StatusCode::OK, Json(serde_json::json!(snapshot))).into_response(),
        None => {
            let error = serde_json::json!({
                "error": format!("No usage data found for provider '{}'", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn get_fallback_chain_handler(
    Path(alias): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let chain = state.model_router.get_fallback_chain(&alias).await;
    Json(serde_json::json!({
        "alias": alias,
        "fallback_chain": chain,
    }))
}

#[allow(dead_code)]
pub async fn set_fallback_chain_handler(
    Path(alias): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SetFallbackChainRequest>,
) -> impl IntoResponse {
    match state
        .model_router
        .set_fallback_chain(&alias, body.providers)
        .await
    {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Fallback chain updated for '{}'", alias),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

pub async fn list_models_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let entries = state.model_router.model_catalog.list().await;
    Json(serde_json::json!({
        "models": entries,
    }))
}

#[allow(dead_code)]
pub async fn get_default_model_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let default = state.model_router.get_default_model().await;
    Json(serde_json::json!({
        "default_model": default,
    }))
}

/// `GET /v1/models`
///
/// Returns available model aliases in OpenAI wire format.
pub async fn openai_list_models_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let entries = state.model_router.model_catalog.list().await;
    let data: Vec<_> = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.id.clone(),
                "object": "model",
                "created": 0,
                "owned_by": entry.provider.clone(),
            })
        })
        .collect();

    Json(serde_json::json!({ "object": "list", "data": data }))
}
