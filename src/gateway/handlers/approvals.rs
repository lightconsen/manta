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

// ── Tool approval management (human-in-the-loop) ──────────────────────────────

#[allow(dead_code)]
/// `GET /api/v1/approvals` — list all pending approval requests.
pub async fn list_approvals_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let approvals = state.tools.approval_queue
        .list_pending(ApprovalFilter::default())
        .await;
    Json(serde_json::json!({ "approvals": approvals, "count": approvals.len() }))
}

#[allow(dead_code)]
/// `GET /api/v1/approvals/:id` — get a specific pending approval.
pub async fn get_approval_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.tools.approval_queue.get(&id).await {
        Some(approval) => Json(approval).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Approval '{}' not found", id) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/approvals/:id/approve` — approve a pending tool call.
pub async fn approve_tool_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    if state.tools.approval_queue
        .resolve(&id, ApprovalDecision::Approve)
        .await
    {
        Json(serde_json::json!({ "id": id, "status": "approved" })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Approval '{}' not found", id) })),
        )
            .into_response()
    }
}

#[allow(dead_code)]
/// `POST /api/v1/approvals/:id/deny` — deny a pending tool call.
pub async fn deny_tool_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    body: Option<Json<DenyApprovalRequest>>,
) -> impl IntoResponse {
    let reason = body
        .and_then(|b| b.reason.clone())
        .unwrap_or_else(|| "Denied by operator".to_string());

    if state.tools.approval_queue
        .resolve(&id, ApprovalDecision::Deny { reason: reason.clone() })
        .await
    {
        Json(serde_json::json!({ "id": id, "status": "denied", "reason": reason })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Approval '{}' not found", id) })),
        )
            .into_response()
    }
}
