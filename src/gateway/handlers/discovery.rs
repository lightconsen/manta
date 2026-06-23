use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tracing::info;

use crate::gateway::GatewayState;

/// Outcome of attempting to spawn a discovered agent from the registry.
#[derive(Debug)]
enum SpawnOutcome {
    Spawned,
    NotFound,
    Failed(String),
}

/// Shared logic for spawning a discovered agent from the registry.
///
/// Delegates to `spawn_agent_inner` so discovered agents get the same
/// lifecycle wiring (self-repair loop, planner store, perception adapter,
/// cron scheduler hook, task registry entry) as gateway-spawned agents.
async fn spawn_discovered_agent(state: &Arc<GatewayState>, id: &str) -> SpawnOutcome {
    // Look up personality in registry
    let personality = {
        let registry = state.agents.registry.read().await;
        registry.get(id).cloned()
    };

    let Some(personality) = personality else {
        return SpawnOutcome::NotFound;
    };

    let config = personality.to_agent_config();

    match crate::gateway::agent_spawn::spawn_agent_inner(state.clone(), id.to_string(), config)
        .await
    {
        Ok(()) => {
            info!("Spawned discovered agent '{}' from registry", id);
            SpawnOutcome::Spawned
        }
        Err(e) => SpawnOutcome::Failed(e.to_string()),
    }
}

#[allow(dead_code)]
/// Handler to spawn a discovered agent from the registry
pub async fn spawn_discovered_agent_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    info!("API request to spawn discovered agent: {}", id);

    match spawn_discovered_agent(&state, &id).await {
        SpawnOutcome::Spawned => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "agent_id": id,
                "status": "spawned",
                "source": "registry",
            })),
        )
            .into_response(),
        SpawnOutcome::NotFound => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("Agent '{}' not found in registry", id),
            })),
        )
            .into_response(),
        SpawnOutcome::Failed(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("Failed to spawn agent: {}", e),
            })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// Handler to spawn all discovered agents
pub async fn spawn_all_discovered_agents_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    info!("API request to spawn all discovered agents");

    let agent_ids: Vec<String> = {
        let registry = state.agents.registry.read().await;
        registry.list()
    };

    let mut spawned = 0;
    let mut failed = 0;

    for agent_id in agent_ids {
        match spawn_discovered_agent(&state, &agent_id).await {
            SpawnOutcome::Spawned => spawned += 1,
            SpawnOutcome::NotFound | SpawnOutcome::Failed(_) => failed += 1,
        }
    }

    info!("Spawned {} agents, {} failed", spawned, failed);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "spawned": spawned,
            "failed": failed,
        })),
    )
        .into_response()
}

#[allow(dead_code)]
/// Handler to list discovered agents in registry
pub async fn list_discovered_agents_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    // Acquire locks in the same order as list_agents_handler to avoid deadlock.
    let (personalities, running_ids): (Vec<_>, std::collections::HashSet<String>) = {
        let agents = state.agents.agents.read().await;
        let registry = state.agents.registry.read().await;
        let personalities: Vec<_> = registry
            .iter()
            .map(|p| (p.id.clone(), p.display_name(), p.is_valid))
            .collect();
        let running_ids: std::collections::HashSet<String> = agents.keys().cloned().collect();
        (personalities, running_ids)
    };

    let list: Vec<_> = personalities
        .into_iter()
        .map(|(id, name, is_valid)| {
            serde_json::json!({
                "id": id,
                "name": name,
                "running": running_ids.contains(&id),
                "valid": is_valid,
            })
        })
        .collect();

    Json(list)
}
