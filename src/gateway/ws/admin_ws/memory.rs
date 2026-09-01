//! WS admin handlers: memory.

use std::sync::Arc;

use super::super::{parse_params, WsRequest, WsResponse};
use crate::gateway::GatewayState;

// ── Memory search / add (vector memory admin) ───────────────────────────

/// `memory.search` — `{ query, limit?, collection?, threshold? }`.
pub(crate) async fn handle_memory_search(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let body: crate::gateway::types::MemorySearchRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state.memory.vector.read().await.clone() {
        Some(vm) => match vm
            .search_collection(&body.query, body.limit, &body.collection, body.threshold)
            .await
        {
            Ok(results) => WsResponse::ok(
                &req.id,
                serde_json::json!({ "query": body.query, "results": results, "count": results.len() }),
            ),
            Err(e) => WsResponse::err(&req.id, "INTERNAL", format!("Search failed: {}", e)),
        },
        None => WsResponse::err(&req.id, "UNAVAILABLE", "Vector memory service not enabled"),
    }
}

/// `memory.add` — `{ content, metadata?, collection? }`.
pub(crate) async fn handle_memory_add(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let body: crate::gateway::types::MemoryAddRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state.memory.vector.read().await.clone() {
        Some(vm) => match vm
            .add_to_collection(&body.content, body.metadata, &body.collection)
            .await
        {
            Ok(doc_id) => WsResponse::ok(
                &req.id,
                serde_json::json!({ "document_id": doc_id, "status": "added" }),
            ),
            Err(e) => {
                WsResponse::err(&req.id, "INTERNAL", format!("Failed to add document: {}", e))
            }
        },
        None => WsResponse::err(&req.id, "UNAVAILABLE", "Vector memory service not enabled"),
    }
}

/// `memory.collections` — list vector memory collections.
pub(crate) async fn handle_memory_collections(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    match state.memory.vector.read().await.clone() {
        Some(vm) => {
            let collections = vm.list_collections().await;
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "collections": collections, "count": collections.len() }),
            )
        }
        None => WsResponse::err(&req.id, "UNAVAILABLE", "Vector memory service not enabled"),
    }
}
