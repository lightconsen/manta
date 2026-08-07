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
                .search_collection(&body.query, body.limit, &body.collection, body.threshold)
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
            let collections = vm.list_collections().await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    /// With `memory.vector == None` (the make_test_state default), all three
    /// handlers must 503 with a "not enabled" message.
    #[tokio::test]
    async fn search_503_when_vector_not_enabled() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let req = MemorySearchRequest {
            query: "foo".into(),
            limit: 5,
            collection: String::new(),
            threshold: 0.5,
        };
        let (status, body) = body_json(
            memory_search_handler(State(state), Json(req))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("not enabled"));
    }

    #[tokio::test]
    async fn add_503_when_vector_not_enabled() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let req = MemoryAddRequest {
            content: "remember me".into(),
            metadata: None,
            collection: String::new(),
        };
        let (status, body) = body_json(
            memory_add_handler(State(state), Json(req))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("not enabled"));
    }

    #[tokio::test]
    async fn list_collections_503_when_vector_not_enabled() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            list_memory_collections_handler(State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("not enabled"));
    }
}
