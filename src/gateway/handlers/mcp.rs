use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Json},
};
use tracing::warn;

use crate::gateway::GatewayState;
use crate::gateway::*;
use crate::mcp::McpServerConfig;

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
///
/// OAuth-aware: when the server uses `auth_type = "oauth2"` and no valid token
/// is stored, starts the OAuth flow and returns `401` with an `auth_url` for
/// the caller to open in a browser. Once authorized, retry this endpoint.
pub async fn connect_mcp_server_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
    Json(body): Json<McpConnectRequest>,
) -> impl IntoResponse {
    use crate::mcp::{McpClient, McpServerConfig, McpTransport};

    let transport = match body.transport.as_str() {
        "sse" => McpTransport::Sse,
        "streamable_http" => McpTransport::StreamableHttp,
        _ => McpTransport::Stdio,
    };

    let config = McpServerConfig {
        transport,
        command: body.command,
        args: body.args,
        env: body.env,
        url: body.url,
        timeout_secs: body.timeout_secs,
        max_tools: body.max_tools,
        auto_connect: body.auto_connect,
        auth_type: body.auth_type,
        client_id: body.client_id,
        auth_url: body.auth_url,
        token_url: body.token_url,
        scopes: body.scopes,
        ..Default::default()
    };

    // OAuth servers: require a stored token before connecting.
    if config.auth_type.as_deref() == Some("oauth2") {
        if !state.tools.mcp_manager.has_stored_token(&server_id).await {
            // No valid stored token — start the browser OAuth flow.
            match state
                .tools
                .mcp_manager
                .start_oauth_flow(&server_id, &config)
                .await
            {
                Ok(auth_url) => {
                    return (
                        axum::http::StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "error": "auth_required",
                            "server_id": server_id,
                            "auth_url": auth_url,
                        })),
                    );
                }
                Err(e) => {
                    return (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": e.to_string() })),
                    );
                }
            }
        }

        // Valid token: connect a client pre-authenticated with the access token.
        let tokens = state.tools.mcp_manager.load_stored_token(&server_id).await;
        if let Some(tokens) = tokens {
            let mut client = McpClient::new().with_timeout(config.timeout_secs);
            client.set_access_token(tokens.access_token.clone());
            match client.connect(config.clone()).await {
                Ok(()) => {
                    let tools = client.get_tools().to_vec();
                    let client_arc = Arc::new(tokio::sync::RwLock::new(client));
                    if let Err(e) = state
                        .tools
                        .mcp_manager
                        .register_client(&server_id, client_arc, config.clone())
                        .await
                    {
                        return (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({ "error": e.to_string() })),
                        );
                    }
                    super::super::lifecycle::register_mcp_tools(
                        &state,
                        &server_id,
                        &tools,
                        config.max_tools,
                    )
                    .await;
                    persist_mcp_config(&state, &server_id, config).await;
                    return (
                        axum::http::StatusCode::OK,
                        Json(serde_json::json!({
                            "server_id": server_id,
                            "tool_count": tools.len(),
                            "tools": tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
                        })),
                    );
                }
                Err(e) => {
                    return (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": e.to_string() })),
                    );
                }
            }
        }
    }

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
            persist_mcp_config(&state, &server_id, config).await;
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

/// Persist an MCP server config to config.toml (best-effort).
async fn persist_mcp_config(
    state: &Arc<GatewayState>,
    server_id: &str,
    config: McpServerConfig,
) {
    {
        let mut cfg_guard = state.config.write().await;
        Arc::make_mut(&mut cfg_guard)
            .mcp
            .servers
            .insert(server_id.to_string(), config);
    }
    if let Some(ref config_path) = state.config_path {
        let cfg_guard = state.config.read().await;
        if let Err(e) = super::config::persist_config_atomic(&cfg_guard, config_path).await {
            warn!("MCP server connected but failed to persist config: {}", e);
        }
    }
}

/// Check whether OAuth authorization has completed for a server.
pub async fn mcp_auth_status_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
) -> impl IntoResponse {
    let authorized = state.tools.mcp_manager.has_stored_token(&server_id).await;
    Json(serde_json::json!({
        "server_id": server_id,
        "authorized": authorized,
    }))
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
