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

// ─────────────────────────────────────────────
// MCP REST API handlers (9.5)
// ─────────────────────────────────────────────

#[allow(dead_code)]
/// List connected MCP servers
pub async fn list_mcp_servers_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let servers = state.mcp_manager.list_servers().await;
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

    match state.mcp_manager.connect(&server_id, config).await {
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
    match state.mcp_manager.disconnect(&server_id).await {
        Ok(()) => {
            // Remove all `mcp__{server_id}__*` tools from the registry so
            // they are no longer offered to agents.
            let prefix = format!("mcp__{server_id}__");
            state.tool_registry.deregister_prefix(&prefix);

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
    match state.mcp_manager.get_client(&server_id).await {
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
    match state.mcp_manager.get_client(&server_id).await {
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
    match state.mcp_manager.get_client(&server_id).await {
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
    match state.mcp_manager.get_client(&server_id).await {
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

// ─────────────────────────────────────────────
// 9.9 – Syscity as an MCP server
// ─────────────────────────────────────────────

/// Expose Syscity's tool registry as an MCP server via the Streamable-HTTP transport.
///
/// Handles JSON-RPC 2.0 requests sent to `POST /mcp`.  Supported methods:
/// - `initialize` – returns server capabilities
/// - `tools/list` – lists all registered tools
/// - `tools/call` – calls a registered tool
///
/// The response content-type is `text/event-stream` (SSE) when the caller
/// sends `Accept: text/event-stream`, or plain `application/json` otherwise.
pub async fn syscity_as_mcp_server_handler(
    State(state): State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::http::header;

    // Parse the incoming JSON-RPC request.
    let request: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return json_rpc_error_response(None, -32700, &format!("Parse error: {}", e));
        }
    };

    let id = request.get("id").cloned();
    let method = match request["method"].as_str() {
        Some(m) => m.to_string(),
        None => {
            return json_rpc_error_response(id.as_ref(), -32600, "Invalid request: missing method");
        }
    };

    let result: serde_json::Value = match method.as_str() {
        "initialize" => {
            let tools = state.tool_registry.get_definitions();
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "syscity",
                    "version": crate::VERSION,
                },
                "capabilities": {
                    "tools": { "count": tools.len() }
                }
            })
        }

        "tools/list" => {
            let defs = state.tool_registry.get_definitions();
            let tools: Vec<serde_json::Value> = defs
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "name": d.name,
                        "description": d.description,
                        "inputSchema": d.parameters,
                    })
                })
                .collect();
            serde_json::json!({ "tools": tools })
        }

        "tools/call" => {
            let params = &request["params"];
            let tool_name = match params["name"].as_str() {
                Some(n) => n.to_string(),
                None => {
                    return json_rpc_error_response(id.as_ref(), -32602, "Missing tool name");
                }
            };
            let args = params["arguments"].clone();

            let context = crate::tools::ToolContext::default();
            match state
                .tool_registry
                .execute(&tool_name, args, &context)
                .await
            {
                Some(Ok(exec_result)) => {
                    serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": exec_result.output,
                        }]
                    })
                }
                Some(Err(e)) => {
                    return json_rpc_error_response(
                        id.as_ref(),
                        -32603,
                        &format!("Tool error: {}", e),
                    );
                }
                None => {
                    return json_rpc_error_response(
                        id.as_ref(),
                        -32601,
                        &format!("Tool not found: {}", tool_name),
                    );
                }
            }
        }

        _ => {
            return json_rpc_error_response(
                id.as_ref(),
                -32601,
                &format!("Method not found: {}", method),
            );
        }
    };

    let response_json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });

    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if accept.contains("text/event-stream") {
        // Respond as SSE
        let sse_body = format!("data: {}\n\n", response_json);
        axum::response::Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(axum::body::Body::from(sse_body))
            .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
    } else {
        axum::response::Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(response_json.to_string()))
            .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
    }
}

