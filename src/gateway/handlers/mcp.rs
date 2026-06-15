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

// ─────────────────────────────────────────────
// MCP REST API handlers (9.5)
// ─────────────────────────────────────────────

#[allow(dead_code)]
/// List connected MCP servers
pub async fn list_mcp_servers_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let servers = state.tools.mcp_manager.list_servers().await;
    Json(serde_json::json!({
        "servers": servers,
        "count": servers.len(),
    }))
}

#[allow(dead_code)]
pub fn mcp_default_timeout() -> u64 {
    30
}

#[allow(dead_code)]
/// Connect to an MCP server
pub async fn connect_mcp_server_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
    Json(body): Json<McpConnectRequest>,
) -> impl IntoResponse {
    use crate::tools::mcp::{McpServerConfig, McpTransport};

    let transport = match body.transport.as_str() {
        "sse" => McpTransport::Sse,
        "streamable_http" => McpTransport::StreamableHttp,
        _ => McpTransport::Stdio,
    };

    let config = McpServerConfig {
        transport,
        command: body.command,
        args: body.args,
        url: body.url,
        timeout_secs: body.timeout_secs,
        ..Default::default()
    };

    match state.tools.mcp_manager.connect(&server_id, config).await {
        Ok(tools) => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({
                "server_id": server_id,
                "tool_count": tools.len(),
                "tools": tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
            })),
        ),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

#[allow(dead_code)]
/// Disconnect from an MCP server
pub async fn disconnect_mcp_server_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
) -> impl IntoResponse {
    match state.tools.mcp_manager.disconnect(&server_id).await {
        Ok(()) => {
            // Remove all `mcp__{server_id}__*` tools from the registry so
            // they are no longer offered to agents.
            let prefix = format!("mcp__{server_id}__");
            state.tools.registry.deregister_prefix(&prefix);

            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({ "disconnected": server_id })),
            )
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

#[allow(dead_code)]
/// List tools from an MCP server
pub async fn list_mcp_tools_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
) -> impl IntoResponse {
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            let tools = client.get_tools().to_vec();
            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({
                    "server_id": server_id,
                    "tools": tools,
                    "count": tools.len(),
                })),
            )
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("MCP server '{}' not found", server_id) })),
        ),
    }
}

#[allow(dead_code)]
/// Call an MCP tool
pub async fn call_mcp_tool_handler(
    State(state): State<Arc<GatewayState>>,
    Path((server_id, tool_name)): Path<(String, String)>,
    Json(args): Json<serde_json::Value>,
) -> impl IntoResponse {
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client.call_tool(&tool_name, args).await {
                Ok(result) => {
                    (axum::http::StatusCode::OK, Json(serde_json::json!({ "result": result })))
                }
                Err(e) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                ),
            }
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("MCP server '{}' not found", server_id) })),
        ),
    }
}

#[allow(dead_code)]
/// List resources from an MCP server
pub async fn list_mcp_resources_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
) -> impl IntoResponse {
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client.list_resources().await {
                Ok(resources) => (
                    axum::http::StatusCode::OK,
                    Json(serde_json::json!({
                        "server_id": server_id,
                        "resources": resources,
                        "count": resources.len(),
                    })),
                ),
                Err(e) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                ),
            }
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("MCP server '{}' not found", server_id) })),
        ),
    }
}

#[allow(dead_code)]
/// Read a resource from an MCP server
pub async fn read_mcp_resource_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
    Json(body): Json<McpReadResourceRequest>,
) -> impl IntoResponse {
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client.read_resource(&body.uri).await {
                Ok(contents) => (
                    axum::http::StatusCode::OK,
                    Json(serde_json::json!({
                        "uri": body.uri,
                        "contents": contents,
                    })),
                ),
                Err(e) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                ),
            }
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("MCP server '{}' not found", server_id) })),
        ),
    }
}
