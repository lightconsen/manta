use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Json},
};
use tracing::warn;

use crate::gateway::GatewayState;
use crate::gateway::*;

// ─────────────────────────────────────────────
// MCP REST API handlers (9.5)
// ─────────────────────────────────────────────

/// List connected MCP servers
pub async fn list_mcp_servers_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let servers = state.tools.mcp_manager.list_servers().await;
    Json(serde_json::json!({
        "servers": servers,
        "count": servers.len(),
    }))
}

/// Connect to an MCP server and persist config.
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
        max_tools: body.max_tools,
        ..Default::default()
    };

    // Connect before persisting — if connection fails, don't save bad config.
    match state
        .tools
        .mcp_manager
        .connect(&server_id, config.clone())
        .await
    {
        Ok(tools) => {
            super::super::lifecycle::register_mcp_tools(&state, &server_id, &tools, body.max_tools)
                .await;

            // Persist to config.toml so the server reconnects on daemon restart.
            {
                let mut cfg_guard = state.config.write().await;
                let cfg = Arc::make_mut(&mut cfg_guard);
                cfg.mcp.servers.insert(server_id.clone(), config);
            }
            if let Some(ref config_path) = state.config_path {
                let cfg_guard = state.config.read().await;
                if let Err(e) = super::config::persist_config_atomic(&cfg_guard, config_path).await
                {
                    warn!("MCP server connected but failed to persist config: {}", e);
                }
            }

            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({
                    "server_id": server_id,
                    "tool_count": tools.len(),
                    "tools": tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
                })),
            )
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// Disconnect from an MCP server and remove from persisted config.
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

            // Remove from persisted config.toml.
            {
                let mut cfg_guard = state.config.write().await;
                Arc::make_mut(&mut cfg_guard).mcp.servers.remove(&server_id);
            }
            if let Some(config_path) = state.config_path.clone() {
                let cfg_guard = state.config.read().await;
                if let Err(e) = super::config::persist_config_atomic(&cfg_guard, &config_path).await
                {
                    warn!("MCP server disconnected but failed to persist config: {}", e);
                }
            }

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
