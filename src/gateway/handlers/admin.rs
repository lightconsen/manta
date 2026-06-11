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

// ── Comprehensive reload ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ReloadRequest {
    #[serde(default = "default_reload_scope")]
    pub scope: String,
}

fn default_reload_scope() -> String {
    "all".to_string()
}

/// Comprehensive reload handler — reloads plugins, config, providers,
/// MCP servers, and skills without requiring a daemon restart.
#[allow(dead_code)]
pub async fn reload_all_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ReloadRequest>,
) -> impl IntoResponse {
    let scope = req.scope.to_lowercase();
    let mut result = serde_json::json!({ "scope": &scope });

    // ── Snapshot pre-reload config for audit diff ──────────────────────
    let pre_snapshot = state.config.read().await.snapshot();

    // ── 1. Reload main configuration from disk ────────────────────────────
    let new_config = if scope == "all" || scope == "config" || scope == "providers" || scope == "mcp" {
        let config_path = state.config_path.clone()
            .unwrap_or_else(|| crate::dirs::syscity_dir().join("syscity.toml"));

        if config_path.exists() {
            match tokio::fs::read_to_string(&config_path).await {
                Ok(content) => match toml::from_str::<crate::gateway::GatewayConfig>(&content) {
                    Ok(cfg) => {
                        info!("Reloaded configuration from {:?}", config_path);
                        Some(cfg)
                    }
                    Err(e) => {
                        error!("Failed to parse syscity.toml: {}", e);
                        None
                    }
                },
                Err(e) => {
                    error!("Failed to read syscity.toml: {}", e);
                    None
                }
            }
        } else {
            warn!("Config file not found at {:?}", config_path);
            None
        }
    } else {
        None
    };

    // ── 2. Plugins ────────────────────────────────────────────────────────
    if scope == "all" || scope == "plugins" {
        let plugins = state.plugin_manager.list_plugins().await;
        let ids: Vec<String> = plugins.iter().map(|p| p.id().to_string()).collect();
        let mut unloaded = 0usize;
        for id in &ids {
            match state.plugin_manager.unload_plugin(id).await {
                Ok(_) => unloaded += 1,
                Err(e) => warn!("Failed to unload plugin '{}' during reload: {}", id, e),
            }
        }
        let loaded = match state.plugin_manager.initialize().await {
            Ok(count) => count,
            Err(e) => {
                error!("Failed to initialize plugins: {}", e);
                0
            }
        };
        result["plugins"] = serde_json::json!({
            "unloaded": unloaded,
            "loaded": loaded,
        });
    }

    // ── 3. Config fields (hot-reloadable subset) ──────────────────────────
    if scope == "all" || scope == "config" {
        if let Some(ref new_cfg) = new_config {
            let mut config = state.config.write().await;
            config.security = new_cfg.security.clone();
            config.providers = new_cfg.providers.clone();
            config.mcp = new_cfg.mcp.clone();
            config.hot_reload = new_cfg.hot_reload.clone();
            config.cost_guard = new_cfg.cost_guard.clone();
            config.capabilities = new_cfg.capabilities.clone();
            config.computer = new_cfg.computer.clone();
            config.workspace_dir = new_cfg.workspace_dir.clone();
            config.workspace_only = new_cfg.workspace_only;
            config.model = new_cfg.model.clone();
            config.model_provider = new_cfg.model_provider.clone();
            config.dreaming = new_cfg.dreaming.clone();
            config.standing_orders = new_cfg.standing_orders.clone();
            config.cron = new_cfg.cron.clone();
            config.browser = new_cfg.browser.clone();
            drop(config);
            result["config"] = serde_json::json!({ "updated": true });
            info!("Applied hot-reloadable configuration fields");
        } else {
            result["config"] = serde_json::json!({ "updated": false, "reason": "parse or read error" });
        }
    }

    // ── Compute config diff and log to audit ──────────────────────────
    if scope == "all" || scope == "config" {
        let post_config = state.config.read().await;
        let changes = post_config.diff_since(&pre_snapshot);
        drop(post_config);

        if !changes.is_empty() {
            let details = serde_json::to_value(&changes).unwrap_or_default();
            state
                .audit_log
                .log(
                    crate::security::runtime_audit::AuditEventType::ConfigChange,
                    "system",
                    "config",
                    true,
                    format!("Config reloaded: {} field(s) changed", changes.len()),
                    Some(details),
                )
                .await;
            info!(
                changes = ?changes.iter().map(|c| &c.path).collect::<Vec<_>>(),
                "Config changes detected on reload"
            );
        }
    }

    // ── 4. Providers sync ─────────────────────────────────────────────────
    if scope == "all" || scope == "providers" {
        let (new_providers, current_names) = if let Some(ref new_cfg) = new_config {
            let new_names: std::collections::HashSet<String> = new_cfg.providers.keys().cloned().collect();
            let current = state.model_router.list_providers().await;
            let current_names: std::collections::HashSet<String> = current.iter().map(|p| p.name.clone()).collect();
            (new_names, current_names)
        } else {
            (std::collections::HashSet::new(), std::collections::HashSet::new())
        };

        let mut added = 0usize;
        let mut removed = 0usize;

        // Remove providers that no longer exist in config
        for name in &current_names {
            if !new_providers.contains(name) {
                if let Err(e) = state.model_router.remove_provider(name).await {
                    warn!("Failed to remove provider '{}': {}", name, e);
                } else {
                    removed += 1;
                    info!("Removed provider '{}' (no longer in config)", name);
                }
            }
        }

        // Add or update providers from new config
        if let Some(ref new_cfg) = new_config {
            for (name, provider_config) in &new_cfg.providers {
                if !current_names.contains(name) {
                    if let Err(e) = state.model_router.add_provider(name, provider_config.clone()).await {
                        warn!("Failed to add provider '{}': {}", name, e);
                    } else {
                        added += 1;
                        info!("Added provider '{}'", name);
                    }
                }
            }
        }

        result["providers"] = serde_json::json!({
            "added": added,
            "removed": removed,
        });
    }

    // ── 5. MCP servers ────────────────────────────────────────────────────
    if scope == "all" || scope == "mcp" {
        // Disconnect all existing MCP servers
        let existing_servers = state.mcp_manager.list_servers().await;
        for server_id in &existing_servers {
            // Deregister tools first
            state.tool_registry.deregister_prefix(&format!("mcp__{}__", server_id));
            if let Err(e) = state.mcp_manager.disconnect(server_id).await {
                warn!("Failed to disconnect MCP server '{}': {}", server_id, e);
            } else {
                info!("Disconnected MCP server '{}'", server_id);
            }
        }

        // Reconnect from new config
        let mut connected = 0usize;
        let mut failed = 0usize;
        if let Some(ref new_cfg) = new_config {
            for (server_id, server_config) in &new_cfg.mcp.servers {
                if !server_config.auto_connect {
                    continue;
                }
                match state.mcp_manager.connect(server_id, server_config.clone()).await {
                    Ok(tools) => {
                        info!(
                            "MCP server '{}' connected: {} tool(s)",
                            server_id,
                            tools.len()
                        );
                        // Register discovered tools
                        if let Some(client_arc) = state.mcp_manager.get_client(server_id).await {
                            let max_tools = if server_config.max_tools == 0 {
                                tools.len()
                            } else {
                                server_config.max_tools.min(tools.len())
                            };
                            for tool in tools.iter().take(max_tools) {
                                let wrapper = Arc::new(McpToolWrapper::new(
                                    client_arc.clone(),
                                    server_id,
                                    tool,
                                ));
                                state.tool_registry.register_dynamic(wrapper);
                            }
                        }
                        connected += 1;
                    }
                    Err(e) => {
                        warn!("Failed to connect MCP server '{}': {}", server_id, e);
                        failed += 1;
                    }
                }
            }
        }

        result["mcp"] = serde_json::json!({
            "disconnected": existing_servers.len(),
            "connected": connected,
            "failed": failed,
        });
    }

    // ── 6. Skills ─────────────────────────────────────────────────────────
    if scope == "all" || scope == "skills" {
        let skills_result = {
            let mut skills_manager = state.skills_manager.write().await;
            match skills_manager.initialize().await {
                Ok(count) => {
                    info!("Reinitialized skills manager with {} skills", count);
                    serde_json::json!({ "reinitialized": true, "count": count })
                }
                Err(e) => {
                    warn!("Failed to reinitialize skills manager: {}", e);
                    serde_json::json!({ "reinitialized": false, "error": e.to_string() })
                }
            }
        };
        result["skills"] = skills_result;
    }

    // ── 7. Channels (document only — rely on file watcher for live reload) ─
    if scope == "all" || scope == "channels" {
        result["channels"] = serde_json::json!({
            "note": "Channels are hot-reloaded automatically when syscity.toml or channel config files change. Use the file watcher or restart individual channels via API.",
        });
    }

    result["success"] = serde_json::json!(true);
    (StatusCode::OK, Json(result)).into_response()
}

