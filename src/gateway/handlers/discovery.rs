use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::agent::Agent;
use crate::gateway::GatewayState;
use crate::gateway::*;

#[allow(dead_code)]
/// Handler to spawn a discovered agent from the registry
pub async fn spawn_discovered_agent_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    info!("API request to spawn discovered agent: {}", id);

    // Check if agent is already running
    {
        let agents = state.agents.agents.read().await;
        if agents.contains_key(&id) {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": format!("Agent '{}' is already running", id),
                    "agent_id": id,
                })),
            )
                .into_response();
        }
    }

    // Check if agent is in registry
    {
        let registry = state.agents.registry.read().await;
        if !registry.has(&id) {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Agent '{}' not found in registry", id),
                    "available_agents": registry.list(),
                })),
            )
                .into_response();
        }
    }

    // Spawn the agent
    // Note: This requires access to the Gateway, so we need to spawn manually
    let personality = {
        let registry = state.agents.registry.read().await;
        registry.get(&id).cloned()
    };

    if let Some(personality) = personality {
        let mut config = personality.to_agent_config();
        config.agent_id = Some(id.clone());

        // Create provider from model router
        let provider = match state.infra.model_router.create_default_provider().await {
            Ok(p) => p,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("Failed to create provider: {}", e),
                    })),
                )
                    .into_response();
            }
        };

        let tools = state.tools.registry.clone();
        let model = state.config.read().await.model.clone();
        let memory_manager = state.memory.manager.read().await.as_ref().cloned();
        let (tx, mut rx) = mpsc::channel(100);

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

        let (query_tx, mut query_rx) = mpsc::channel::<AgentQuery>(32);

        let handle = AgentHandle {
            id: id.clone(),
            config: config.clone(),
            tx: tx.clone(),
            query_tx: query_tx.clone(),
            busy: false,
            agent: agent.clone(),
        };

        {
            let mut agents = state.agents.agents.write().await;
            agents.insert(id.clone(), handle);
        }

        // Start agent processing loop
        let state_clone = state.clone();
        let agent_id_clone = id.clone();
        tokio::spawn(async move {
            info!("Agent {} processing loop started", agent_id_clone);
            loop {
                tokio::select! {
                    cmd = rx.recv() => {
                    let cmd = match cmd { Some(c) => c, None => break };
                    match cmd {
                        AgentCommand::Shutdown => {
                            info!("Agent {} shutting down", agent_id_clone);
                            let _ = state_clone.events.tx.send(GatewayEvent::AgentStatus {
                                agent_id: agent_id_clone.clone(),
                                status: AgentStatus::Shutdown,
                            });
                            break;
                        }
                        AgentCommand::ProcessMessage {
                            session_id,
                            message,
                            user_id,
                            channel,
                            model_override,
                        } => {
                            let incoming_msg = crate::channels::IncomingMessage::new(
                                user_id.clone(),
                                session_id.clone(),
                                message.clone(),
                            );

                            agent.set_model_override(model_override).await;
                            let result = agent.process_message(incoming_msg).await;
                            agent.set_model_override(None).await;

                            match result {
                                Ok(outgoing) => {
                                    // Route response back to channel
                                    let _ = state_clone.events.tx.send(GatewayEvent::AgentResponse {
                                        session_id: session_id.clone(),
                                        agent_id: agent_id_clone.clone(),
                                        content: outgoing.content,
                                        channel: channel.clone(),
                                        conversation_id: session_id.clone(),
                                        usage: outgoing.usage,
                                    });
                                }
                                Err(e) => {
                                    error!("Agent {} failed to process message: {}", agent_id_clone, e);
                                }
                            }
                        }
                        _ => {
                            info!("Agent {} received command: {:?}", agent_id_clone, cmd);
                        }
                    }
                    } // cmd = rx.recv() arm
                    query = query_rx.recv() => {
                        let query = match query { Some(q) => q, None => break };
                        match query {
                            AgentQuery::GetThreadSummaries { response_tx } => {
                                let _ = response_tx.send(agent.thread_summaries().await);
                            }
                            AgentQuery::GetThreadTurns { conv_id, response_tx } => {
                                let _ = response_tx.send(agent.thread_turns_for(&conv_id).await);
                            }
                            AgentQuery::UndoLastTurn { conv_id, response_tx } => {
                                let _ = response_tx.send(agent.undo_last_turn(&conv_id).await);
                            }
                            AgentQuery::RedoLastTurn { conv_id, response_tx } => {
                                let _ = response_tx.send(agent.redo_last_turn(&conv_id).await);
                            }
                            AgentQuery::RunSkill { session_id, message, user_id, skill_trust, response_tx } => {
                                agent.set_skill_trust(skill_trust);
                                let incoming = crate::channels::IncomingMessage::new(
                                    user_id, &session_id, message,
                                );
                                let no_op: crate::agent::ProgressCallback =
                                    Arc::new(|_| Box::pin(async {}));
                                let result = agent.process_message_with_progress(incoming, no_op).await;
                                agent.set_skill_trust(crate::tools::SkillTrust::Trusted);
                                let _ = response_tx.send(result);
                            }
                        }
                    }
                }
            } // end tokio::select! and loop
            info!("Agent {} processing loop ended", agent_id_clone);
        });

        info!("✅ Spawned discovered agent '{}' from registry", id);
        (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "agent_id": id,
                "status": "spawned",
                "source": "registry",
            })),
        )
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("Agent '{}' not found in registry", id),
            })),
        )
            .into_response()
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
    let mut already_running = 0;
    let mut failed = 0;

    for agent_id in agent_ids {
        // Check if already running
        {
            let agents = state.agents.agents.read().await;
            if agents.contains_key(&agent_id) {
                already_running += 1;
                continue;
            }
        }

        // Spawn the agent
        let personality = {
            let registry = state.agents.registry.read().await;
            registry.get(&agent_id).cloned()
        };

        if let Some(personality) = personality {
            let mut config = personality.to_agent_config();
            config.agent_id = Some(agent_id.clone());

            if let Ok(provider) = state.infra.model_router.create_default_provider().await {
                let tools = state.tools.registry.clone();
                let model = state.config.read().await.model.clone();
                let memory_manager = state.memory.manager.read().await.as_ref().cloned();
                let (tx, mut rx) = mpsc::channel(100);

                let agent = if let Some(mm) = memory_manager {
                    Arc::new(
                        Agent::new(config.clone(), provider, tools)
                            .with_model(model)
                            .with_memory_manager(mm)
                            .with_transcript_store(Arc::clone(&state.infra.transcript_store))
                            .with_artifact_store(Arc::clone(&state.infra.artifact_store))
                            .with_disk_budget(Arc::clone(&state.infra.disk_budget))
                            .with_session_file_manager(Arc::clone(
                                &state.infra.session_file_manager,
                            ))
                            .with_skill_manager(Arc::clone(&state.tools.skills_manager)),
                    )
                } else {
                    Arc::new(
                        Agent::new(config.clone(), provider, tools)
                            .with_model(model)
                            .with_transcript_store(Arc::clone(&state.infra.transcript_store))
                            .with_artifact_store(Arc::clone(&state.infra.artifact_store))
                            .with_disk_budget(Arc::clone(&state.infra.disk_budget))
                            .with_session_file_manager(Arc::clone(
                                &state.infra.session_file_manager,
                            ))
                            .with_skill_manager(Arc::clone(&state.tools.skills_manager)),
                    )
                };

                let (query_tx, mut query_rx) = mpsc::channel::<AgentQuery>(32);

                let handle = AgentHandle {
                    id: agent_id.clone(),
                    config: config.clone(),
                    tx: tx.clone(),
                    query_tx: query_tx.clone(),
                    busy: false,
                    agent: agent.clone(),
                };

                {
                    let mut agents = state.agents.agents.write().await;
                    agents.insert(agent_id.clone(), handle);
                }

                // Start processing loop
                let state_clone = state.clone();
                let agent_id_clone = agent_id.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            cmd = rx.recv() => {
                                let cmd = match cmd { Some(c) => c, None => break };
                                if let AgentCommand::Shutdown = cmd { break; }
                            }
                            query = query_rx.recv() => {
                                let query = match query { Some(q) => q, None => break };
                                match query {
                                    AgentQuery::GetThreadSummaries { response_tx } => {
                                        let _ = response_tx.send(agent.thread_summaries().await);
                                    }
                                    AgentQuery::GetThreadTurns { conv_id, response_tx } => {
                                        let _ = response_tx.send(agent.thread_turns_for(&conv_id).await);
                                    }
                                    AgentQuery::UndoLastTurn { conv_id, response_tx } => {
                                        let _ = response_tx.send(agent.undo_last_turn(&conv_id).await);
                                    }
                                    AgentQuery::RedoLastTurn { conv_id, response_tx } => {
                                        let _ = response_tx.send(agent.redo_last_turn(&conv_id).await);
                                    }
                                    AgentQuery::RunSkill { session_id, message, user_id, skill_trust, response_tx } => {
                                        agent.set_skill_trust(skill_trust);
                                        let incoming = crate::channels::IncomingMessage::new(
                                            user_id, &session_id, message,
                                        );
                                        let no_op: crate::agent::ProgressCallback =
                                            Arc::new(|_| Box::pin(async {}));
                                        let result = agent.process_message_with_progress(incoming, no_op).await;
                                        agent.set_skill_trust(crate::tools::SkillTrust::Trusted);
                                        let _ = response_tx.send(result);
                                    }
                                }
                            }
                        }
                    }
                    let _ = state_clone.events.tx.send(GatewayEvent::AgentStatus {
                        agent_id: agent_id_clone,
                        status: AgentStatus::Shutdown,
                    });
                });

                spawned += 1;
            } else {
                failed += 1;
            }
        } else {
            failed += 1;
        }
    }

    info!(
        "Spawned {} agents, {} already running, {} failed",
        spawned, already_running, failed
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "spawned": spawned,
            "already_running": already_running,
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
    let registry = state.agents.registry.read().await;
    let agents = state.agents.agents.read().await;

    let list: Vec<_> = registry
        .iter()
        .map(|p| {
            let is_running = agents.contains_key(&p.id);
            serde_json::json!({
                "id": p.id,
                "name": p.display_name(),
                "running": is_running,
                "valid": p.is_valid,
            })
        })
        .collect();

    Json(list)
}
