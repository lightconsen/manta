use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::gateway::GatewayState;
use crate::gateway::*;

#[allow(dead_code)]
pub async fn memory_search_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<MemorySearchRequest>,
) -> impl IntoResponse {
    match state.memory.vector.read().await.clone() {
        Some(vm) => {
            match vm
                .search_collection(&body.query, body.limit, &body.collection)
                .await
            {
                Ok(results) => {
                    let response = serde_json::json!({
                        "query": body.query,
                        "results": results,
                        "count": results.len(),
                    });
                    (StatusCode::OK, Json(response)).into_response()
                }
                Err(e) => {
                    let error = serde_json::json!({
                        "error": format!("Search failed: {}", e),
                    });
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
                }
            }
        }
        None => {
            let error = serde_json::json!({
                "error": "Vector memory service not enabled",
            });
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn memory_add_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<MemoryAddRequest>,
) -> impl IntoResponse {
    match state.memory.vector.read().await.clone() {
        Some(vm) => {
            match vm
                .add_to_collection(&body.content, body.metadata, &body.collection)
                .await
            {
                Ok(doc_id) => {
                    let response = serde_json::json!({
                        "document_id": doc_id,
                        "status": "added",
                    });
                    (StatusCode::CREATED, Json(response)).into_response()
                }
                Err(e) => {
                    let error = serde_json::json!({
                        "error": format!("Failed to add document: {}", e),
                    });
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
                }
            }
        }
        None => {
            let error = serde_json::json!({
                "error": "Vector memory service not enabled",
            });
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn list_memory_collections_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.memory.vector.read().await.clone() {
        Some(vm) => {
            let collections = vm.list_collections();
            Json(serde_json::json!({
                "collections": collections,
                "count": collections.len(),
            }))
            .into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": "Vector memory service not enabled",
            });
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        }
    }
}
