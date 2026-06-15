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

#[allow(dead_code)]
/// Get current gateway configuration
pub async fn get_config_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let config = state.config.read().await;
    match serde_json::to_value(&*config) {
        Ok(json) => (StatusCode::OK, Json(json)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Serialization failed: {}", e)})),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// Update gateway configuration and persist to disk
pub async fn put_config_handler(
    State(state): State<Arc<GatewayState>>,
    Json(new_config): Json<GatewayConfig>,
) -> impl IntoResponse {
    let config_path = match state.config_path.clone() {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(
                    serde_json::json!({"error": "No config file path configured — cannot persist changes"}),
                ),
            )
                .into_response();
        }
    };

    // Serialize to TOML
    let toml_str = match toml::to_string_pretty(&new_config) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("TOML serialization failed: {}", e)})),
            )
                .into_response();
        }
    };

    // Write to disk
    if let Err(e) = tokio::fs::write(&config_path, toml_str).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to write config file: {}", e)})),
        )
            .into_response();
    }

    // Update in-memory config
    {
        let mut config = state.config.write().await;
        *config = new_config;
    }

    info!("Config updated and persisted to {:?}", config_path);

    state.auth.audit_log
        .log(
            crate::security::runtime_audit::AuditEventType::ConfigChange,
            "admin",
            "gateway",
            true,
            format!("Config updated and persisted to {}", config_path.display()),
            Some(serde_json::json!({"path": config_path.to_string_lossy()})),
        )
        .await;

    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "updated", "path": config_path.to_string_lossy()})),
    )
        .into_response()
}

#[allow(dead_code)]
/// Validate a configuration without persisting it
pub async fn validate_config_handler(Json(config): Json<GatewayConfig>) -> impl IntoResponse {
    // Basic validation: try to serialize and deserialize as TOML
    match toml::to_string(&config) {
        Ok(toml_str) => {
            match toml::from_str::<GatewayConfig>(&toml_str) {
                Ok(_) => (
                    StatusCode::OK,
                    Json(serde_json::json!({"valid": true, "message": "Configuration is valid"})),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"valid": false, "error": format!("TOML deserialization failed: {}", e)})),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"valid": false, "error": format!("TOML serialization failed: {}", e)})),
        )
            .into_response(),
    }
}
