//! WS admin handlers: mcp.

use std::sync::Arc;

use super::super::{parse_params, WsRequest, WsResponse};
use crate::gateway::GatewayState;

// ── MCP ─────────────────────────────────────────────────────────────────

/// `mcp.tools` — list a server's tools (`{ server_id }`).
pub(crate) async fn handle_mcp_tools(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let server_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["server_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            let tools = client.get_tools().to_vec();
            WsResponse::ok(&req.id, serde_json::json!({ "tools": tools }))
        }
        None => WsResponse::err(&req.id, "NOT_FOUND", "MCP server not connected"),
    }
}

/// `mcp.call_tool` — invoke a tool on a connected server (`{ server_id, tool, args }`).
pub(crate) async fn handle_mcp_call_tool(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let server_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["server_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let tool_name = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["tool"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let args = req.params.clone().and_then(|p| p["args"].clone().into());
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client
                .call_tool(&tool_name, args.unwrap_or(serde_json::json!({})))
                .await
            {
                Ok(result) => WsResponse::ok(&req.id, serde_json::json!({ "result": result })),
                Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
            }
        }
        None => {
            WsResponse::err(&req.id, "NOT_FOUND", format!("MCP server '{}' not found", server_id))
        }
    }
}

/// `mcp.resources` — list a server's resources (`{ server_id }`).
pub(crate) async fn handle_mcp_resources(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let server_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["server_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client.list_resources().await {
                Ok(resources) => {
                    WsResponse::ok(&req.id, serde_json::json!({ "resources": resources }))
                }
                Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
            }
        }
        None => WsResponse::err(&req.id, "NOT_FOUND", "MCP server not connected"),
    }
}

/// `mcp.auth_status` — whether a server has a stored OAuth token (`{ server_id }`).
pub(crate) async fn handle_mcp_auth_status(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let server_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["server_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let authorized = state.tools.mcp_manager.has_stored_token(&server_id).await;
    WsResponse::ok(&req.id, serde_json::json!({ "server_id": server_id, "authorized": authorized }))
}
