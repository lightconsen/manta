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

// Skills API Handlers

#[allow(dead_code)]
pub async fn list_skills_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let skills_manager = state.skills_manager.read().await;
    let skills = skills_manager.list_skills().await;

    let skill_list: Vec<_> = skills
        .iter()
        .map(|skill| {
            serde_json::json!({
                "id": skill.name.clone(),
                "name": skill.name.clone(),
                "description": skill.description.clone(),
                "enabled": skill.enabled,
                "is_eligible": skill.is_eligible,
                "triggers": skill.triggers.iter().map(|t| format!("{:?}", t.trigger_type)).collect::<Vec<_>>(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "skills": skill_list,
        "count": skill_list.len(),
    }))
}

#[allow(dead_code)]
pub async fn get_skill_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let skills_manager = state.skills_manager.read().await;
    match skills_manager.get_skill(&id).await {
        Some(skill) => {
            let response = serde_json::json!({
                "id": id,
                "name": skill.name,
                "description": skill.description,
                "enabled": skill.enabled,
                "is_eligible": skill.is_eligible,
                "triggers": skill.triggers.iter().map(|t| format!("{:?}", t.trigger_type)).collect::<Vec<_>>(),
                "eligibility_errors": skill.eligibility_errors,
            });
            Json(response).into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": format!("Skill '{}' not found", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn enable_skill_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let mut skills_manager = state.skills_manager.write().await;
    match skills_manager.set_skill_enabled(&id, true).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Skill '{}' enabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to enable skill: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn disable_skill_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let mut skills_manager = state.skills_manager.write().await;
    match skills_manager.set_skill_enabled(&id, false).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Skill '{}' disabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to disable skill: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn run_skill_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<RunSkillRequest>,
) -> impl IntoResponse {
    let skills_manager = state.skills_manager.read().await;

    // Activate skill with runtime requirement verification
    let skill = match skills_manager.activate_skill(&id).await {
        Ok(s) => s,
        Err(crate::error::SyscityError::NotFound { .. }) => {
            let error = serde_json::json!({
                "error": format!("Skill '{}' not found", id),
            });
            return (StatusCode::NOT_FOUND, Json(error)).into_response();
        }
        Err(crate::error::SyscityError::Validation(msg)) => {
            // Requirements not met at activation time
            let error = serde_json::json!({
                "error": "Skill requirements not met",
                "details": msg,
                "skill_id": id,
            });
            return (StatusCode::PRECONDITION_FAILED, Json(error)).into_response();
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to activate skill '{}': {}", id, e),
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response();
        }
    };

    if !skill.enabled {
        let error = serde_json::json!({
            "error": format!("Skill '{}' is disabled", id),
        });
        return (StatusCode::BAD_REQUEST, Json(error)).into_response();
    }

    // Note: is_eligible is checked at load time, but activate_skill() also
    // verifies requirements at runtime. If we got here, requirements are met.

    // Build the message: skill system prompt + user input
    let full_message = if skill.prompt.is_empty() {
        body.input.clone()
    } else {
        format!("{}\n\nUser input: {}", skill.prompt, body.input)
    };

    // Capture trust level before dropping the lock (skill is owned so this is just being explicit)
    let skill_trust = skill.metadata.trust;

    // Drop read lock before acquiring agents lock
    drop(skills_manager);

    // Get the default agent's query channel to execute the skill
    let query_tx = {
        let agents = state.agents.read().await;
        agents.get("default").map(|h| h.query_tx.clone())
    };

    let query_tx = match query_tx {
        Some(tx) => tx,
        None => {
            let error = serde_json::json!({
                "error": "No default agent available to run skill",
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response();
        }
    };

    // Execute via actor channel
    let session_id = format!("skill-{}-{}", id, uuid::Uuid::new_v4());
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if query_tx
        .send(AgentQuery::RunSkill {
            session_id: session_id.clone(),
            message: full_message,
            user_id: "skill-runner".to_string(),
            skill_trust,
            response_tx: resp_tx,
        })
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
        Ok(Ok(outgoing)) => {
            let response = serde_json::json!({
                "skill_id": id,
                "session_id": session_id,
                "status": "completed",
                "result": outgoing.content,
                "usage": outgoing.usage,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Ok(Err(e)) => {
            let error = serde_json::json!({
                "error": format!("Skill execution failed: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "agent response channel closed"})),
        )
            .into_response(),
    }
}

