use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tokio::sync::mpsc;

use crate::agent::AgentConfig;
use crate::gateway::agent_spawn::run_agent_loop;
use crate::gateway::GatewayState;
use crate::gateway::*;

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
                "busy": handle.busy.load(std::sync::atomic::Ordering::Relaxed),
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

    use crate::agent::Agent;

    // Generate unique agent ID
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4());
    info!("Creating new agent via API: {}", agent_id);

    let mut config = config;
    config.agent_id = Some(agent_id.clone());

    // Create communication channel
    let (tx, rx) = mpsc::channel(100);

    // Create provider from model router
    let provider = match state.infra.model_router.create_default_provider().await {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to create provider: {}", e)
                })),
            )
                .into_response();
        }
    };

    // Get tools, model, and memory manager
    let tools = state.tools.registry.clone();
    let model = state.config.read().await.model.clone();
    let memory_manager = state.memory.manager.read().await.as_ref().cloned();

    // Create agent instance with memory manager and session management stores
    let agent = if let Some(mm) = memory_manager {
        Arc::new(
            Agent::new(config.clone(), provider, tools)
                .with_model(model)
                .with_memory_manager(mm)
                .with_transcript_store(Arc::clone(&state.infra.transcript_store))
                .with_artifact_store(Arc::clone(&state.infra.artifact_store))
                .with_disk_budget(Arc::clone(&state.infra.disk_budget))
                .with_session_file_manager(Arc::clone(&state.infra.session_file_manager))
                .with_skill_manager(Arc::clone(&state.tools.skills_manager)),
        )
    } else {
        Arc::new(
            Agent::new(config.clone(), provider, tools)
                .with_model(model)
                .with_transcript_store(Arc::clone(&state.infra.transcript_store))
                .with_artifact_store(Arc::clone(&state.infra.artifact_store))
                .with_disk_budget(Arc::clone(&state.infra.disk_budget))
                .with_session_file_manager(Arc::clone(&state.infra.session_file_manager))
                .with_skill_manager(Arc::clone(&state.tools.skills_manager)),
        )
    };

    let (query_tx, query_rx) = mpsc::channel::<AgentQuery>(32);

    // Create agent handle
    let handle = AgentHandle {
        id: agent_id.clone(),
        config: config.clone(),
        tx: tx.clone(),
        query_tx: query_tx.clone(),
        busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        agent: agent.clone(),
    };
    let handle_for_loop = handle.clone();

    // Insert into agents map
    {
        let mut agents = state.agents.agents.write().await;
        agents.insert(agent_id.clone(), handle);
    }

    // Start agent processing loop (mirrors spawn_agent behavior)
    let task_registry = state.task_registry.clone();
    let agent_id_clone = agent_id.clone();
    let agent_clone = agent.clone();
    let busy = handle_for_loop.busy.clone();
    let state_clone = state.clone();

    let task_handle = tokio::spawn(async move {
        run_agent_loop(state_clone, agent_id_clone, agent_clone, busy, rx, query_rx, false).await;
    });
    task_registry
        .insert_join(format!("agent:{}", agent_id), task_handle)
        .await;

    info!("✅ Agent {} created successfully", agent_id);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": agent_id,
            "status": "created",
            "config": {
                "max_context_tokens": config.max_context_tokens,
                "max_concurrent_tools": config.max_concurrent_tools,
                "temperature": config.temperature,
                "max_tokens": config.max_tokens,
            }
        })),
    )
        .into_response()
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
            "busy": agent.busy.load(std::sync::atomic::Ordering::Relaxed),
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
    let _ = state.events.tx.send(GatewayEvent::AgentStatus {
        agent_id: id.clone(),
        status: AgentStatus::Shutdown,
    });

    info!("✅ Agent {} deleted successfully", id);
    StatusCode::NO_CONTENT
}

#[allow(dead_code)]
pub async fn list_channels_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let channels = state.channels.channels.read().await;
    let list: Vec<_> = channels.keys().cloned().collect();
    Json(list)
}
