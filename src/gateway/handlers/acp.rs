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

// ACP (Agent Control Plane) API Handlers

#[allow(dead_code)]
pub async fn list_acp_sessions_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let subagents = state.acp.list_subagents().await;
    let sessions: Vec<_> = subagents
        .iter()
        .map(|s| {
            serde_json::json!({
                "subagent_id": s.id,
                "session_id": s.session_id.to_string(),
                "parent_id": s.parent_id,
                "mode": format!("{:?}", s.mode),
                "status": format!("{:?}", s.status),
                "thread_id": s.thread_id,
            })
        })
        .collect();

    Json(serde_json::json!({
        "sessions": sessions,
        "count": sessions.len(),
    }))
}

#[allow(dead_code)]
pub async fn acp_spawn_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SpawnSubagentRequest>,
) -> impl IntoResponse {
    use crate::acp::{AcpSessionId, SpawnMode, SubagentConfig, ThreadBinding};
    use crate::channels::IncomingMessage;
    use crate::security::runtime_audit::AuditEventType;
    use crate::security::RateLimitResult;
    use crate::security::UserId;

    // Rate limit: 10 spawns per minute per api-user
    let actor = "api-user";
    let rate_result = state
        .rate_limiter
        .check_with_cost(&UserId::new(format!("acp:spawn:{}", actor)), 1.0)
        .await;
    if !rate_result.is_allowed() {
        let retry = match rate_result {
            RateLimitResult::Denied { retry_after_secs } => retry_after_secs,
            _ => 60,
        };
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "Rate limit exceeded for ACP spawn",
                "retry_after": retry,
            })),
        )
            .into_response();
    }

    let session_id = AcpSessionId::new();
    let parent_id = "gateway-api".to_string();

    let mode = match body.mode.as_str() {
        "session" => SpawnMode::Session,
        _ => SpawnMode::Run,
    };

    let agent_type = if body.agent_type.is_empty() {
        "default".to_string()
    } else {
        body.agent_type.clone()
    };
    let config = SubagentConfig {
        agent_type: agent_type.clone(),
        mode,
        thread_binding: ThreadBinding::Auto,
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        tools: vec![],
        context: None,
        timeout_seconds: Some(300),
        retry_on_crash: false,
        max_crash_retries: 3,
    };

    match state
        .acp
        .spawn_subagent(session_id.clone(), parent_id.clone(), config)
        .await
    {
        Ok(handle) => {
            let subagent_id = handle.id.clone();

            // Audit log
            state
                .audit_log
                .log(
                    AuditEventType::AcpSpawn,
                    actor,
                    &subagent_id,
                    true,
                    format!("Spawned subagent via API (mode: {:?})", handle.mode),
                    Some(serde_json::json!({
                        "session_id": session_id.to_string(),
                        "parent_id": parent_id,
                        "agent_type": agent_type,
                    })),
                )
                .await;

            // Send task to subagent
            let message =
                IncomingMessage::new(actor.to_string(), session_id.to_string(), body.task);

            match state.acp.send_message(&subagent_id, message).await {
                Ok(response) => {
                    let resp = serde_json::json!({
                        "subagent_id": subagent_id,
                        "session_id": session_id.to_string(),
                        "mode": format!("{:?}", handle.mode),
                        "response": response,
                    });
                    (StatusCode::CREATED, Json(resp)).into_response()
                }
                Err(e) => {
                    let _ = state.acp.shutdown_subagent(&subagent_id).await;
                    let error = serde_json::json!({
                        "error": format!("Subagent failed to process task: {}", e),
                    });
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
                }
            }
        }
        Err(e) => {
            // Audit log failed spawn
            state
                .audit_log
                .log(
                    AuditEventType::AcpSpawn,
                    actor,
                    "",
                    false,
                    format!("Failed to spawn subagent: {}", e),
                    None,
                )
                .await;

            let error = serde_json::json!({
                "error": format!("Failed to spawn subagent: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn terminate_acp_session_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    use crate::acp::AcpSessionId;
    use crate::security::runtime_audit::AuditEventType;

    let session_id = AcpSessionId(id.clone());
    match state.acp.terminate_session(&session_id).await {
        Ok(count) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpTerminate,
                    "api-user",
                    &id,
                    true,
                    format!("Terminated {} subagents in session {}", count, id),
                    Some(serde_json::json!({ "terminated_count": count })),
                )
                .await;
            let response = serde_json::json!({
                "terminated_count": count,
                "session_id": session_id.to_string(),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpTerminate,
                    "api-user",
                    &id,
                    false,
                    format!("Failed to terminate session: {}", e),
                    None,
                )
                .await;
            let error = serde_json::json!({
                "error": format!("Failed to terminate session: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn acp_session_message_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<AcpMessageRequest>,
) -> impl IntoResponse {
    use crate::acp::AcpSessionId;
    use crate::channels::IncomingMessage;
    use crate::security::runtime_audit::AuditEventType;

    // Find a subagent in this session
    let session_id = AcpSessionId(id.clone());
    let subagents = state.acp.list_session_subagents(&session_id).await;

    if subagents.is_empty() {
        let error = serde_json::json!({
            "error": "No active subagents in session",
        });
        return (StatusCode::NOT_FOUND, Json(error)).into_response();
    }

    // Use the first active subagent
    let subagent = &subagents[0];
    let message =
        IncomingMessage::new("api-user".to_string(), session_id.to_string(), body.message);

    match state.acp.send_message(&subagent.id, message).await {
        Ok(response) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpMessage,
                    "api-user",
                    &id,
                    true,
                    format!("Message sent to subagent {} in session {}", subagent.id, id),
                    Some(serde_json::json!({
                        "subagent_id": subagent.id,
                        "session_id": id,
                    })),
                )
                .await;
            let resp = serde_json::json!({
                "subagent_id": subagent.id,
                "session_id": session_id.to_string(),
                "response": response,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpMessage,
                    "api-user",
                    &id,
                    false,
                    format!("Failed to send message: {}", e),
                    None,
                )
                .await;
            let error = serde_json::json!({
                "error": format!("Failed to send message: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
/// Get ACP session runtime status
pub async fn acp_session_status_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.acp.get_status(id.clone()).await {
        Some(status) => {
            let resp = serde_json::json!({
                "session_id": status.session_id,
                "runtime_state": format!("{}", status.runtime_state),
                "mode": format!("{:?}", status.mode),
                "current_iteration": status.current_iteration,
                "max_iterations": status.max_iterations,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": "Session not found",
                "session_id": id,
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
/// Pause an ACP session
pub async fn acp_session_pause_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    state.acp.pause(id.clone()).await;
    let resp = serde_json::json!({
        "session_id": id,
        "action": "pause",
        "status": "requested",
    });
    (StatusCode::OK, Json(resp)).into_response()
}

#[allow(dead_code)]
/// Resume a paused ACP session
pub async fn acp_session_resume_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    state.acp.resume(id.clone()).await;
    let resp = serde_json::json!({
        "session_id": id,
        "action": "resume",
        "status": "requested",
    });
    (StatusCode::OK, Json(resp)).into_response()
}

#[allow(dead_code)]
/// Single-step a paused ACP session
pub async fn acp_session_step_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    state.acp.step(id.clone()).await;
    let resp = serde_json::json!({
        "session_id": id,
        "action": "step",
        "status": "requested",
    });
    (StatusCode::OK, Json(resp)).into_response()
}

#[allow(dead_code)]
/// Cancel a running ACP session
pub async fn acp_session_cancel_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    state.acp.cancel(id.clone()).await;
    let resp = serde_json::json!({
        "session_id": id,
        "action": "cancel",
        "status": "requested",
    });
    (StatusCode::OK, Json(resp)).into_response()
}

#[allow(dead_code)]
/// Get subagent tree for an ACP session
pub async fn acp_session_tree_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let session_id = crate::acp::AcpSessionId(id.clone());
    let tree = state.acp.get_subagent_tree(&session_id).await;

    let resp = serde_json::json!({
        "session_id": id,
        "tree": tree,
    });
    (StatusCode::OK, Json(resp)).into_response()
}

#[allow(dead_code)]
/// Execute a message in ACP session mode (persistent context)
pub async fn acp_execute_session_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<AcpExecuteRequest>,
) -> impl IntoResponse {
    let agent_id = body.agent_id.unwrap_or_else(|| "default".to_string());
    let agents = state.agents.read().await;
    let agent_handle = match agents.get(&agent_id) {
        Some(h) => h.clone(),
        None => {
            let error = serde_json::json!({
                "error": format!("Agent '{}' not found", agent_id),
            });
            return (StatusCode::NOT_FOUND, Json(error)).into_response();
        }
    };
    drop(agents);

    let session_id = uuid::Uuid::new_v4().to_string();
    let incoming = crate::channels::IncomingMessage::new(
        body.user_id.clone(),
        session_id.clone(),
        body.message,
    );

    match state
        .acp
        .execute_session(agent_handle.agent, incoming)
        .await
    {
        Ok(outgoing) => {
            let resp = serde_json::json!({
                "session_id": session_id,
                "mode": "session",
                "response": outgoing.content,
                "usage": outgoing.usage,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Execution failed: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
/// Execute a message in ACP run mode (one-shot, no persistence)
pub async fn acp_execute_run_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<AcpExecuteRequest>,
) -> impl IntoResponse {
    let agent_id = body.agent_id.unwrap_or_else(|| "default".to_string());
    let agents = state.agents.read().await;
    let agent_handle = match agents.get(&agent_id) {
        Some(h) => h.clone(),
        None => {
            let error = serde_json::json!({
                "error": format!("Agent '{}' not found", agent_id),
            });
            return (StatusCode::NOT_FOUND, Json(error)).into_response();
        }
    };
    drop(agents);

    let session_id = uuid::Uuid::new_v4().to_string();
    let incoming = crate::channels::IncomingMessage::new(
        body.user_id.clone(),
        session_id.clone(),
        body.message,
    );

    match state.acp.execute_run(agent_handle.agent, incoming).await {
        Ok(outgoing) => {
            let resp = serde_json::json!({
                "session_id": session_id,
                "mode": "run",
                "response": outgoing.content,
                "usage": outgoing.usage,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Execution failed: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

