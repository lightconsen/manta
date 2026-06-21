use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tokio::sync::mpsc;
use tracing::error;

use crate::agent::AgentConfig;
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
                "busy": handle.busy,
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
    let (tx, mut rx) = mpsc::channel(100);

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

    let (query_tx, mut query_rx) = mpsc::channel::<AgentQuery>(32);

    // Create agent handle
    let handle = AgentHandle {
        id: agent_id.clone(),
        config: config.clone(),
        tx: tx.clone(),
        query_tx: query_tx.clone(),
        busy: false,
        agent: agent.clone(),
    };

    // Insert into agents map
    {
        let mut agents = state.agents.agents.write().await;
        agents.insert(agent_id.clone(), handle);
    }

    // Start agent processing loop (mirrors spawn_agent behavior)
    let state_clone = state.clone();
    let agent_id_clone = agent_id.clone();
    let agent_clone = agent.clone();
    tokio::spawn(async move {
        info!("Agent {} processing loop started", agent_id_clone);
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                let cmd = match cmd { Some(c) => c, None => break };
                match cmd {
                    AgentCommand::ProcessMessage { session_id, message, user_id, channel, model_override } => {
                        let source_channel = channel;
                        info!("Agent {} processing message for session {}", agent_id_clone, session_id);

                        // Update status
                        let _ = state_clone.events.tx.send(GatewayEvent::AgentStatus {
                            agent_id: agent_id_clone.clone(),
                            status: AgentStatus::Processing { session_id: session_id.clone() },
                        });

                        // Create incoming message
                        let incoming_msg = crate::channels::IncomingMessage::new(
                            user_id.clone(), session_id.clone(), message.clone()
                        );

                        // Process with progress callbacks
                        let progress_state = state_clone.clone();
                        let progress_session = session_id.clone();
                        let progress_agent = agent_id_clone.clone();
                        let progress_cb: crate::agent::ProgressCallback = Arc::new(move |event| {
                            let state = progress_state.clone();
                            let sid = progress_session.clone();
                            let aid = progress_agent.clone();
                            Box::pin(async move {
                                // Read directive settings
                                let reasoning_vis = {
                                    let s = state.infra.runtime_settings.read().await;
                                    s.get("reasoning.visibility").and_then(|v| v.as_str()).map(|s| s.to_string())
                                };
                                let verbose_mode = {
                                    let s = state.infra.runtime_settings.read().await;
                                    s.get("verbose.mode").and_then(|v| v.as_str()).map(|s| s.to_string())
                                };
                                match event {
                                    crate::agent::ProgressEvent::Started => {
                                        let _ = state.events.tx.send(GatewayEvent::AgentStatus {
                                            agent_id: aid.clone(),
                                            status: AgentStatus::Processing { session_id: sid.clone() },
                                        });
                                    }
                                    crate::agent::ProgressEvent::Generating { content } => {
                                        if reasoning_vis.as_deref() == Some("off") {
                                            return;
                                        }
                                        // Only emit thinking events when there's actual content
                                        if let Some(ref thinking) = content {
                                            if !thinking.is_empty() {
                                                let _ = state.events.tx.send(GatewayEvent::Thinking {
                                                    session_id: sid.clone(),
                                                    agent_id: aid.clone(),
                                                    content: Some(thinking.clone()),
                                                });
                                            }
                                        }
                                    }
                                    crate::agent::ProgressEvent::ContentDelta { text } => {
                                        let _ = state.events.tx.send(GatewayEvent::ContentDelta {
                                            session_id: sid.clone(),
                                            agent_id: aid.clone(),
                                            delta: text,
                                        });
                                    }
                                    crate::agent::ProgressEvent::ToolCalling { name, arguments } => {
                                        if verbose_mode.as_deref() == Some("off") {
                                            return;
                                        }
                                        let _ = state.events.tx.send(GatewayEvent::ToolCalling {
                                            session_id: sid.clone(), agent_id: aid.clone(),
                                            tool_name: name.clone(), arguments: arguments.clone(),
                                        });
                                    }
                                    crate::agent::ProgressEvent::ToolResult { name, result, data } => {
                                        if verbose_mode.as_deref() == Some("off") {
                                            return;
                                        }
                                        let result = if verbose_mode.as_deref() == Some("compact") {
                                            if result.len() > 500 {
                                                format!("{}... (truncated)", &result[..500])
                                            } else {
                                                result
                                            }
                                        } else {
                                            result
                                        };
                                        let _ = state.events.tx.send(GatewayEvent::ToolResult {
                                            session_id: sid.clone(), agent_id: aid.clone(),
                                            tool_name: name.clone(), result, data,
                                        });
                                    }
                                    crate::agent::ProgressEvent::ToolResultDelta { .. } => {
                                        // Streaming tool chunks are accumulated locally and emitted
                                        // as a final ToolResult event; no per-chunk gateway event yet.
                                    }
                                    crate::agent::ProgressEvent::Completed { response } => {
                                        let _ = state.events.tx.send(GatewayEvent::Completed {
                                            session_id: sid.clone(),
                                            agent_id: aid.clone(),
                                            response,
                                        });
                                    }
                                    crate::agent::ProgressEvent::Error { message } => {
                                        let _ = state.events.tx.send(GatewayEvent::ProcessingError {
                                            session_id: sid.clone(),
                                            agent_id: aid.clone(),
                                            message,
                                        });
                                    }
                                }
                            })
                        });

                        // Apply thinking config from runtime settings
                        let think_level = {
                            let s = state_clone.infra.runtime_settings.read().await;
                            s.get("think.level").and_then(|v| v.as_str()).map(|s| s.to_string())
                        };
                        let extra = think_level.and_then(|level| {
                            let budget = match level.as_str() {
                                "minimal" => 1024u32,
                                "low" => 4096u32,
                                "medium" => 16000u32,
                                "high" => 32000u32,
                                _ => return None,
                            };
                            Some(serde_json::json!({ "thinking": { "type": "enabled", "budget_tokens": budget } }))
                        });
                        agent_clone.set_model_override(model_override).await;
                        agent_clone.set_extra_params(extra).await;

                        let result = agent_clone.process_message_with_progress(incoming_msg, progress_cb).await;
                        agent_clone.set_model_override(None).await;

                        match result {
                            Ok(mut outgoing) => {
                                // Apply reasoning visibility filter
                                let reasoning_vis = {
                                    let s = state_clone.infra.runtime_settings.read().await;
                                    s.get("reasoning.visibility").and_then(|v| v.as_str()).map(|s| s.to_string())
                                };
                                if reasoning_vis.as_deref() == Some("off") {
                                    outgoing.reasoning_content = None;
                                }
                                // Accumulate usage
                                if let Some(ref usage) = outgoing.usage {
                                    let mut settings = state_clone.infra.runtime_settings.write().await;
                                    let current_tokens = settings.get("usage.tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                    let total_tokens = usage.prompt_tokens as u64 + usage.completion_tokens as u64;
                                    settings.insert("usage.tokens".to_string(), serde_json::json!(current_tokens + total_tokens));
                                    let current_calls = settings.get("usage.calls").and_then(|v| v.as_u64()).unwrap_or(0);
                                    let tool_calls = outgoing.tool_calls.as_ref().map(|c| c.len() as u64).unwrap_or(0);
                                    settings.insert("usage.calls".to_string(), serde_json::json!(current_calls + tool_calls + 1));
                                }
                                let _ = state_clone.events.tx.send(GatewayEvent::AgentResponse {
                                    session_id: session_id.clone(), agent_id: agent_id_clone.clone(),
                                    content: outgoing.content, channel: source_channel.clone(),
                                    conversation_id: session_id.clone(), usage: outgoing.usage,
                                });
                            }
                            Err(e) => {
                                error!("Agent {} failed to process: {}", agent_id_clone, e);
                            }
                        }

                        let _ = state_clone.events.tx.send(GatewayEvent::AgentStatus {
                            agent_id: agent_id_clone.clone(), status: AgentStatus::Idle,
                        });
                    }
                    AgentCommand::Shutdown => {
                        info!("Agent {} shutting down", agent_id_clone);
                        let _ = state_clone.events.tx.send(GatewayEvent::AgentStatus {
                            agent_id: agent_id_clone.clone(), status: AgentStatus::Shutdown,
                        });
                        break;
                    }
                    _ => info!("Agent {} received command: {:?}", agent_id_clone, cmd),
                }
                } // cmd = rx.recv() arm
                query = query_rx.recv() => {
                    let query = match query { Some(q) => q, None => break };
                    match query {
                        AgentQuery::GetThreadSummaries { response_tx } => {
                            let _ = response_tx.send(agent_clone.thread_summaries().await);
                        }
                        AgentQuery::GetThreadTurns { conv_id, response_tx } => {
                            let _ = response_tx.send(agent_clone.thread_turns_for(&conv_id).await);
                        }
                        AgentQuery::UndoLastTurn { conv_id, response_tx } => {
                            let _ = response_tx.send(agent_clone.undo_last_turn(&conv_id).await);
                        }
                        AgentQuery::RedoLastTurn { conv_id, response_tx } => {
                            let _ = response_tx.send(agent_clone.redo_last_turn(&conv_id).await);
                        }
                        AgentQuery::RunSkill { session_id, message, user_id, skill_trust, response_tx } => {
                            agent_clone.set_skill_trust(skill_trust);
                            let incoming = crate::channels::IncomingMessage::new(
                                user_id, &session_id, message,
                            );
                            let no_op: crate::agent::ProgressCallback =
                                Arc::new(|_| Box::pin(async {}));
                            let result =
                                agent_clone.process_message_with_progress(incoming, no_op).await;
                            agent_clone.set_skill_trust(crate::tools::SkillTrust::Trusted);
                            let _ = response_tx.send(result);
                        }
                    }
                }
            }
        } // end tokio::select! and loop
        info!("Agent {} processing loop ended", agent_id_clone);
    });

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
            "busy": agent.busy,
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
