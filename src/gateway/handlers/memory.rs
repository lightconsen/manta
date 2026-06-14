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
pub async fn memory_search_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<MemorySearchRequest>,
) -> impl IntoResponse {
    let vector_memory = state.vector_memory.read().await;
    match vector_memory.as_ref() {
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
    let vector_memory = state.vector_memory.read().await;
    match vector_memory.as_ref() {
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
    let vector_memory = state.vector_memory.read().await;
    match vector_memory.as_ref() {
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
