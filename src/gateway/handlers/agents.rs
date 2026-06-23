use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::agent::AgentConfig;
use crate::gateway::runtime::{AgentCommand, AgentStatus, GatewayEvent};
use crate::gateway::GatewayState;

#[allow(dead_code)]
pub async fn list_agents_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    // Get running agents
    let running_agents = state.agents.agents.read().await;

    // Get discovered personalities from registry
    let registry = state.agents.registry.read().await;
    let discovered: Vec<_> = registry.iter().map(|p| p.id.clone()).collect();

    let list: Vec<_> = running_agents
        .iter()
        .map(|(id, handle)| {
            let is_discovered = discovered.contains(id);
            serde_json::json!({
                "id": id,
                "busy": handle.busy.load(std::sync::atomic::Ordering::Acquire),
                "status": "running",
                "discovered": is_discovered,
            })
        })
        .collect();

    // Add discovered but not running agents
    let not_running: Vec<_> = discovered
        .into_iter()
        .filter(|id| !running_agents.contains_key(id))
        .map(|id| {
            serde_json::json!({
                "id": id,
                "busy": false,
                "status": "discovered",
                "discovered": true,
            })
        })
        .collect();

    let combined: Vec<_> = list.into_iter().chain(not_running).collect();
    Json(combined)
}

#[allow(dead_code)]
pub async fn create_agent_handler(
    State(state): State<Arc<GatewayState>>,
    Json(config): Json<AgentConfig>,
) -> impl IntoResponse {
    use tracing::info;

    // Generate unique agent ID
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4());
    info!("Creating new agent via API: {}", agent_id);

    // Extract summary fields before handing config to spawn_agent_inner.
    let response_config = serde_json::json!({
        "max_context_tokens": config.max_context_tokens,
        "max_concurrent_tools": config.max_concurrent_tools,
        "temperature": config.temperature,
        "max_tokens": config.max_tokens,
    });

    match crate::gateway::agent_spawn::spawn_agent_inner(state, agent_id.clone(), config).await {
        Ok(()) => {
            info!("✅ Agent {} created successfully", agent_id);
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "id": agent_id,
                    "status": "created",
                    "config": response_config,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("Failed to create agent: {}", e)
            })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
pub async fn get_agent_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let agents = state.agents.agents.read().await;
    match agents.get(&id) {
        Some(agent) => Json(serde_json::json!({
            "id": agent.id,
            "busy": agent.busy.load(std::sync::atomic::Ordering::Acquire),
        }))
        .into_response(),
        None => (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    }
}

#[allow(dead_code)]
pub async fn delete_agent_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    use tracing::{info, warn};

    info!("Deleting agent via API: {}", id);

    // Check if agent exists
    let agent_exists = {
        let agents = state.agents.agents.read().await;
        agents.contains_key(&id)
    };

    if !agent_exists {
        warn!("Agent {} not found for deletion", id);
        return StatusCode::NOT_FOUND;
    }

    // Get the agent's channel and send shutdown
    let tx = {
        let agents = state.agents.agents.read().await;
        agents.get(&id).map(|h| h.tx.clone())
    };

    if let Some(tx) = tx {
        // Send shutdown command
        if let Err(e) = tx.send(AgentCommand::Shutdown).await {
            warn!("Failed to send shutdown to agent {}: {}", id, e);
        }
    }

    // Remove from agents map
    {
        let mut agents = state.agents.agents.write().await;
        agents.remove(&id);
    }

    // Send event
    if let Err(e) = state.events.tx.send(GatewayEvent::AgentStatus {
        agent_id: id.clone(),
        status: AgentStatus::Shutdown,
    }) {
        warn!("Failed to broadcast agent shutdown event for {}: {}", id, e);
    }

    info!("✅ Agent {} deleted successfully", id);
    StatusCode::NO_CONTENT
}

#[allow(dead_code)]
pub async fn list_channels_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let channels = state.channels.channels.read().await;
    let list: Vec<_> = channels.keys().cloned().collect();
    Json(list)
}
