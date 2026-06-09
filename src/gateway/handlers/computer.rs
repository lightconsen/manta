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

/// Take a screenshot of the desktop.
pub async fn computer_screenshot_handler(
    State(state): State<Arc<GatewayState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let adapter_guard = state.computer_adapter.read().await;
    let adapter = match adapter_guard.as_ref() {
        Some(a) => a.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "Computer adapter not available"
                })),
            )
                .into_response();
        }
    };
    drop(adapter_guard);

    let region = match (params.get("x"), params.get("y"), params.get("w"), params.get("h")) {
        (Some(xs), Some(ys), Some(ws), Some(hs)) => {
            match (xs.parse(), ys.parse(), ws.parse(), hs.parse()) {
                (Ok(x), Ok(y), Ok(w), Ok(h)) => {
                    Some(crate::computer::Rect::new(x, y, w, h))
                }
                _ => None,
            }
        }
        _ => None,
    };

    match adapter.screenshot(region).await {
        Ok(screenshot) => {
            let response = serde_json::json!({
                "success": true,
                "width": screenshot.width,
                "height": screenshot.height,
                "base64": screenshot.base64,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": e.to_string(),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response)).into_response()
        }
    }
}

/// Execute a desktop action.
pub async fn computer_execute_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<ComputerExecuteRequest>,
) -> impl IntoResponse {
    let adapter_guard = state.computer_adapter.read().await;
    let adapter = match adapter_guard.as_ref() {
        Some(a) => a.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "Computer adapter not available"
                })),
            );
        }
    };
    drop(adapter_guard);

    match adapter.execute(body.action).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap_or_default())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "error": e.to_string(),
            })),
        ),
    }
}

/// Get computer adapter status.
pub async fn computer_status_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let adapter_guard = state.computer_adapter.read().await;
    let available = adapter_guard.is_some();
    drop(adapter_guard);

    let config = state.config.read().await;
    let computer_enabled = config.computer.enabled;
    let max_steps = config.computer.max_steps;
    let settle_delay_ms = config.computer.settle_delay_ms;
    let has_display = crate::computer::has_display_server();
    drop(config);

    Json(serde_json::json!({
        "available": available,
        "enabled": computer_enabled,
        "has_display_server": has_display,
        "max_steps": max_steps,
        "settle_delay_ms": settle_delay_ms,
    }))
}

