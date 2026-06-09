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

// ── Session / Thread / Turn API ───────────────────────────────────────────────

#[allow(dead_code)]
/// `GET /api/sessions` — list all active sessions and their routing info.
pub async fn list_sessions_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let bindings = state.agent_router.list_bindings().await;
    let sessions: Vec<_> = bindings
        .iter()
        .map(|(session_id, (agent_id, workspace_id))| {
            serde_json::json!({
                "session_id": session_id,
                "agent_id": agent_id,
                "workspace_id": workspace_id,
            })
        })
        .collect();
    let count = sessions.len();
    Json(serde_json::json!({
        "sessions": sessions,
        "count": count,
    }))
}

#[allow(dead_code)]
/// Resolve session_id → query sender, returning a 404 response on failure.
///
/// The caller must NOT hold any lock when invoking this helper.
pub async fn resolve_session_query_tx(
    state: &Arc<GatewayState>,
    session_id: &str,
) -> Result<mpsc::Sender<AgentQuery>, axum::response::Response> {
    let agent_id = {
        let route = state.agent_router.resolve_by_session(session_id).await;
        if route.agent_id == "default" && route.created_binding {
            // No existing binding and fell back to default — treat as not found
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Session '{}' not found", session_id)
                })),
            )
                .into_response());
        }
        route.agent_id
    };

    let agents = state.agents.read().await;
    match agents.get(&agent_id) {
        Some(handle) => Ok(handle.query_tx.clone()),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("Agent '{}' for session '{}' not found", agent_id, session_id)
            })),
        )
            .into_response()),
    }
}

#[allow(dead_code)]
/// `GET /api/sessions/:id/threads` — list threads for a session's agent.
pub async fn list_threads_handler(
    Path(session_id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let qtx = match resolve_session_query_tx(&state, &session_id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if qtx
        .send(AgentQuery::GetThreadSummaries { response_tx: resp_tx })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "agent unavailable"})),
        )
            .into_response();
    }
    let summaries = match resp_rx.await {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "agent response channel closed"})),
            )
                .into_response()
        }
    };

    let threads: Vec<_> = summaries
        .into_iter()
        .map(|(thread_id, label, turn_count, conv_id)| {
            serde_json::json!({
                "thread_id": thread_id,
                "label": label,
                "turn_count": turn_count,
                "conversation_id": conv_id,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "session_id": session_id,
            "threads": threads,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// `GET /api/sessions/:id/threads/:thread_id/turns` — list turns for a thread.
pub async fn list_turns_handler(
    Path((session_id, thread_id)): Path<(String, String)>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let qtx = match resolve_session_query_tx(&state, &session_id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };

    // Thread map key is `conversation_id`; the CLI passes `thread_id` with a
    // "thread-" prefix. Strip it to get the correct map key.
    let conv_id = thread_id
        .strip_prefix("thread-")
        .unwrap_or(&thread_id)
        .to_string();

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if qtx
        .send(AgentQuery::GetThreadTurns { conv_id, response_tx: resp_tx })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "agent unavailable"})),
        )
            .into_response();
    }
    match resp_rx.await {
        Ok(Some(turns)) => {
            let turns_json: Vec<_> = turns
                .into_iter()
                .map(|(index, turn_state, user_preview, asst_preview)| {
                    serde_json::json!({
                        "index": index,
                        "state": turn_state,
                        "user_preview": user_preview,
                        "assistant_preview": asst_preview,
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "session_id": session_id,
                    "thread_id": thread_id,
                    "turns": turns_json,
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("Thread '{}' not found", thread_id),
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "agent response channel closed"})),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/sessions/:id/threads/:thread_id/undo` — undo the last turn of a thread.
pub async fn undo_turn_handler(
    Path((session_id, thread_id)): Path<(String, String)>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let qtx = match resolve_session_query_tx(&state, &session_id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };

    let conv_id = thread_id
        .strip_prefix("thread-")
        .unwrap_or(&thread_id)
        .to_string();

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if qtx
        .send(AgentQuery::UndoLastTurn { conv_id, response_tx: resp_tx })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "agent unavailable"})),
        )
            .into_response();
    }
    match resp_rx.await {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "session_id": session_id,
                "thread_id": thread_id,
                "message": "Last turn undone successfully",
            })),
        )
            .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "Thread '{}' not found or has no turns to undo",
                    thread_id
                ),
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "agent response channel closed"})),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/sessions/:id/threads/:thread_id/redo` — redo the most recently undone turn.
pub async fn redo_turn_handler(
    Path((session_id, thread_id)): Path<(String, String)>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let qtx = match resolve_session_query_tx(&state, &session_id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };

    let conv_id = thread_id
        .strip_prefix("thread-")
        .unwrap_or(&thread_id)
        .to_string();

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if qtx
        .send(AgentQuery::RedoLastTurn { conv_id, response_tx: resp_tx })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "agent unavailable"})),
        )
            .into_response();
    }
    match resp_rx.await {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "session_id": session_id,
                "thread_id": thread_id,
                "message": "Turn redone successfully",
            })),
        )
            .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "Thread '{}' not found or has no turns to redo",
                    thread_id
                ),
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "agent response channel closed"})),
        )
            .into_response(),
    }
}

