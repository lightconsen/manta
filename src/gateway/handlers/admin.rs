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

// ── Entity management ─────────────────────────────────────────────────────────

#[allow(dead_code)]
/// `GET /api/v1/entities` — list all entities.
pub async fn list_entities_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let storage = state.storage.read().await;
    match storage.list().await {
        Ok(entities) => Json(serde_json::json!({
            "entities": entities,
            "count": entities.len(),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/entities` — create a new entity.
pub async fn create_entity_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<CreateEntityRequest>,
) -> impl IntoResponse {
    use crate::core::models::{Entity, Status};

    let mut entity = Entity::new(req.name);
    if let Some(desc) = req.description {
        entity = entity.with_description(desc);
    }
    if let Some(tags) = req.tags {
        entity = entity.with_tags(tags);
    }
    if let Some(status_str) = req.status {
        if let Ok(s) = status_str.parse::<Status>() {
            entity = entity.with_status(s);
        }
    }

    let storage = state.storage.read().await;
    match storage.create(&entity).await {
        Ok(()) => (StatusCode::CREATED, Json(entity)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `GET /api/v1/entities/:id` — get a single entity.
pub async fn get_entity_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    use crate::core::models::Id;

    let entity_id = match Id::parse(&id) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid ID: {}", e) })),
            )
                .into_response();
        }
    };

    let storage = state.storage.read().await;
    match storage.get(entity_id).await {
        Ok(entity) => Json(entity).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[allow(dead_code)]
/// `PUT /api/v1/entities/:id` — update an entity.
pub async fn update_entity_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<UpdateEntityRequest>,
) -> impl IntoResponse {
    use crate::core::models::{Id, Status};

    let entity_id = match Id::parse(&id) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid ID: {}", e) })),
            )
                .into_response();
        }
    };

    let storage = state.storage.read().await;
    let mut entity = match storage.get(entity_id).await {
        Ok(e) => e,
        Err(e) => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                .into_response();
        }
    };

    if let Some(name) = req.name {
        entity.set_name(name);
    }
    if let Some(desc) = req.description {
        entity.description = Some(desc);
    }
    if let Some(tags) = req.tags {
        entity.tags = Some(tags);
    }
    if let Some(status_str) = req.status {
        if let Ok(s) = status_str.parse::<Status>() {
            entity.status = s;
        }
    }
    entity.metadata.touch();

    match storage.update(&entity).await {
        Ok(()) => Json(entity).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `DELETE /api/v1/entities/:id` — delete an entity.
pub async fn delete_entity_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    use crate::core::models::Id;

    let entity_id = match Id::parse(&id) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid ID: {}", e) })),
            )
                .into_response();
        }
    };

    let storage = state.storage.read().await;
    match storage.delete(entity_id).await {
        Ok(()) => Json(serde_json::json!({ "success": true, "id": id })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/entities/search` — search entities by name.
pub async fn search_entities_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SearchEntitiesRequest>,
) -> impl IntoResponse {
    let storage = state.storage.read().await;
    match storage.list().await {
        Ok(entities) => {
            let query_lower = req.query.to_lowercase();
            let results: Vec<_> = entities
                .into_iter()
                .filter(|e| {
                    e.name.to_lowercase().contains(&query_lower)
                        || e.description
                            .as_deref()
                            .map(|d| d.to_lowercase().contains(&query_lower))
                            .unwrap_or(false)
                })
                .collect();
            Json(serde_json::json!({ "results": results, "count": results.len() })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `GET /api/v1/entities/export` — export all entities as JSON.
pub async fn export_entities_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let storage = state.storage.read().await;
    match storage.list().await {
        Ok(entities) => Json(serde_json::json!({ "entities": entities, "count": entities.len() }))
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/entities/import` — bulk import entities.
pub async fn import_entities_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ImportEntitiesRequest>,
) -> impl IntoResponse {
    use crate::core::models::Entity;

    let storage = state.storage.read().await;
    let mut imported = 0usize;
    let mut errors = Vec::<String>::new();

    for val in req.entities {
        match serde_json::from_value::<Entity>(val) {
            Ok(entity) => match storage.create(&entity).await {
                Ok(()) => imported += 1,
                Err(e) => errors.push(format!("{}: {}", entity.name, e)),
            },
            Err(e) => errors.push(format!("Parse error: {}", e)),
        }
    }

    Json(serde_json::json!({
        "imported": imported,
        "errors": errors,
    }))
    .into_response()
}

// ── Team management ───────────────────────────────────────────────────────────

#[allow(dead_code)]
/// `GET /api/v1/teams` — list all teams.
pub async fn list_teams_handler(_state: State<Arc<GatewayState>>) -> impl IntoResponse {
    match crate::team::Team::list_all().await {
        Ok(names) => {
            Json(serde_json::json!({ "teams": names, "count": names.len() })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/teams` — create a new team.
pub async fn create_team_handler(
    _state: State<Arc<GatewayState>>,
    Json(req): Json<CreateTeamRequest>,
) -> impl IntoResponse {
    let mut team = crate::team::Team::new(req.name.clone());
    team.description = req.description;
    team.active = true;

    match team.save().await {
        Ok(()) => (StatusCode::CREATED, Json(team)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `GET /api/v1/teams/:id` — get team details.
pub async fn get_team_handler(
    Path(id): Path<String>,
    _state: State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(team) => Json(team).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[allow(dead_code)]
/// `DELETE /api/v1/teams/:id` — delete a team.
pub async fn delete_team_handler(
    Path(id): Path<String>,
    _state: State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(team) => match team.delete().await {
            Ok(()) => Json(serde_json::json!({ "success": true, "name": id })).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{}", e) })),
            )
                .into_response(),
        },
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[allow(dead_code)]
/// `GET /api/v1/teams/:id/members` — list team members.
pub async fn list_team_members_handler(
    Path(id): Path<String>,
    _state: State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(team) => {
            let members: Vec<_> = team.members.values().collect();
            Json(serde_json::json!({ "members": members, "count": members.len() })).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[allow(dead_code)]
pub fn default_member_role() -> String {
    "member".to_string()
}

#[allow(dead_code)]
/// `POST /api/v1/teams/:id/members` — add a member to the team.
pub async fn add_team_member_handler(
    Path(id): Path<String>,
    _state: State<Arc<GatewayState>>,
    Json(req): Json<AddTeamMemberRequest>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(mut team) => {
            team.add_member(req.agent.clone(), req.role);
            match team.save().await {
                Ok(()) => Json(serde_json::json!({
                    "success": true,
                    "team": id,
                    "agent": req.agent,
                }))
                .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                )
                    .into_response(),
            }
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[allow(dead_code)]
/// `DELETE /api/v1/teams/:id/members/:agent` — remove a member from the team.
pub async fn remove_team_member_handler(
    Path((id, agent)): Path<(String, String)>,
    _state: State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(mut team) => {
            team.remove_member(&agent);
            match team.save().await {
                Ok(()) => Json(serde_json::json!({
                    "success": true,
                    "team": id,
                    "agent": agent,
                }))
                .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                )
                    .into_response(),
            }
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[allow(dead_code)]
pub fn default_task_priority() -> String {
    "normal".to_string()
}

#[allow(dead_code)]
/// `POST /api/v1/teams/:id/tasks` — assign a task to the team via the mesh.
pub async fn assign_team_task_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AssignTeamTaskRequest>,
) -> impl IntoResponse {
    // Verify team exists
    let team = match crate::team::Team::load(&id).await {
        Ok(t) => t,
        Err(e) => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                .into_response();
        }
    };

    // Route the task through the inbound pipeline using the team as a session
    let session_id = format!("team:{}", id);
    let incoming = crate::channels::IncomingMessage::new(
        format!("team:{}", team.name),
        session_id,
        format!("[priority:{}] {}", req.priority, req.task),
    )
    .with_provenance(crate::channels::InputProvenance::InternalSystem {
        source: "team".to_string(),
    });
    let _ = state.inbound_pipeline.process(incoming).await;

    Json(serde_json::json!({
        "success": true,
        "team": id,
        "task": req.task,
        "priority": req.priority,
        "queued": true,
    }))
    .into_response()
}

