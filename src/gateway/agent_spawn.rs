//! Agent-spawning logic and tool-registry factory.
//!
//! Extracted from `gateway/mod.rs` to reduce the main control-plane file.
//! Re-exported via `pub use agent_spawn::*;` so callers continue to see
//! `spawn_agent_inner` / `create_default_tool_registry` at the `gateway` level.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::session_store::AppendMessageParams;
use crate::agent::{Agent, AgentConfig};
use crate::config::CapabilitiesConfig;
use crate::tools::approval::ApprovalQueue;
use crate::tools::delegate_tool::AgentResolver;
use crate::tools::mcp::McpManager;
use crate::tools::ToolRegistry;

// ── GatewayAgentResolver ─────────────────────────────────────────────────────

/// Wraps the Gateway agent map for [`AgentResolver`] lookups.
pub(crate) struct GatewayAgentResolver {
    pub(crate) agents: Arc<RwLock<std::collections::HashMap<String, super::AgentHandle>>>,
}

#[async_trait]
impl AgentResolver for GatewayAgentResolver {
    async fn resolve(&self, name: &str) -> Option<Arc<Agent>> {
        let agent = {
            let agents = self.agents.read().await;
            agents.get(name).map(|h| h.agent.clone())
        };
        agent
    }
}

// ── spawn_agent_inner ────────────────────────────────────────────────────────

/// Resolve the model an agent should use at spawn time.
///
/// Fast mode stores the active model in `runtime_settings` so that toggling it
/// does not require mutating the immutable config snapshot. If fast mode is not
/// active, fall back to the configured default model.
async fn effective_model_for_spawn(state: &super::GatewayState) -> String {
    let settings = state.infra.runtime_settings.read().await;
    if let Some(model) = settings.get("fast.active_model").and_then(|v| v.as_str()) {
        return model.to_string();
    }
    state.config.read().await.model.clone()
}

/// Spawn a single agent, wire it into the Gateway's agent pool, register its
/// perception adapter and computer adapter, and start the per-agent message
/// processing loop. The loop's JoinHandle is owned by the task registry.
pub(crate) async fn spawn_agent_inner(
    state: Arc<super::GatewayState>,
    id: String,
    mut config: AgentConfig,
) -> crate::Result<()> {
    config.agent_id = Some(id.clone());
    info!("Spawning agent: {}", id);

    // Reserve the agent ID across the async setup to prevent concurrent
    // callers from both passing the duplicate check and creating ghost tasks.
    {
        let mut pending = state
            .agents
            .pending_spawns
            .lock()
            .expect("pending_spawns mutex poisoned");
        if !pending.insert(id.clone()) {
            return Err(crate::SyscityError::Validation(format!(
                "Agent '{}' spawn already in progress",
                id
            )));
        }
    }

    // Ensure the pending entry is cleared on every exit path.
    struct PendingGuard {
        pending: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
        id: String,
    }
    impl Drop for PendingGuard {
        fn drop(&mut self) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&self.id);
            }
        }
    }
    let _pending_guard = PendingGuard {
        pending: state.agents.pending_spawns.clone(),
        id: id.clone(),
    };

    let (tx, rx) = tokio::sync::mpsc::channel(100);

    // Create provider from model router
    let provider: Arc<dyn crate::providers::Provider> =
        state.infra.model_router.create_default_provider().await?;
    // Get tool registry from state
    let tools = state.tools.registry.clone();

    // Get the model for this agent, honoring any active fast-mode override
    // stored in runtime_settings.
    let model = effective_model_for_spawn(&state).await;

    // Create the actual Agent instance with model, memory manager, chat history,
    // shared cost guard, and session management stores.
    let memory_manager = state.memory.manager.read().await.as_ref().cloned();
    let cost_guard = Arc::clone(&state.agents.cost_guard);

    // Read computer config for the agent
    let computer_config = {
        let cfg = state.config.read().await;
        crate::computer::LoopConfig {
            max_steps: cfg.computer.max_steps,
            settle_delay_ms: cfg.computer.settle_delay_ms,
            ..Default::default()
        }
    };
    let computer_adapter = state.tools.computer_adapter.read().await.clone();

    // Mint a per-agent perception adapter if the perception pipeline
    // is initialized. Dispatches to the configured summarizer backend
    // (Template / Local / Llm) and respects the master enable_summary
    // switch so that the default deployment pays zero LLM tokens for
    // the periodic `### Summary` block.
    let perception_adapter: Option<Arc<dyn crate::perception::AgentPerceptionAdapter>> = {
        let init = state.perception_init.read().await;
        let p_cfg = &state.config.read().await.perception;
        // Use the lazily-initialized shared summarizer so the (potentially
        // async, model-downloading) backend is only built once across all
        // agent spawns.
        if let Some(p) = init.as_ref() {
            let summarizer: Option<Arc<dyn crate::perception::PerceptionSummarizer>> =
                if p_cfg.enable_summary {
                    Some(
                        state
                            .summarizer
                            .get_or_init(|| {
                                build_summarizer(
                                    &p_cfg.summarizer_kind,
                                    provider.clone(),
                                    model.clone(),
                                )
                            })
                            .await
                            .clone(),
                    )
                } else {
                    None
                };
            let adapter_cfg = crate::perception::AdapterConfig {
                enable_summary: p_cfg.enable_summary,
                summary_refresh_interval: p_cfg
                    .summary_refresh_secs
                    .or(Some(60))
                    .map(std::time::Duration::from_secs),
                ..Default::default()
            };
            Some(
                p.context
                    .new_adapter(crate::perception::Focus::default(), summarizer, adapter_cfg)
                    as Arc<dyn crate::perception::AgentPerceptionAdapter>,
            )
        } else {
            None
        }
    };

    let agent = if let Some(mm) = memory_manager {
        let chat_history = mm.chat_history();
        let mut builder = Agent::new(config.clone(), provider, tools)
            .with_model(model.clone())
            .with_memory_manager(mm.clone())
            .with_chat_history(chat_history)
            .with_cost_guard(cost_guard)
            .with_transcript_store(Arc::clone(&state.infra.transcript_store))
            .with_artifact_store(Arc::clone(&state.infra.artifact_store))
            .with_disk_budget(Arc::clone(&state.infra.disk_budget))
            .with_session_file_manager(Arc::clone(&state.infra.session_file_manager))
            .with_model_router(Arc::clone(&state.infra.model_router))
            .with_skill_manager(Arc::clone(&state.tools.skills_manager))
            .with_model_alias(model.clone());
        if let Some(adapter) = computer_adapter.clone() {
            builder = builder
                .with_computer_adapter(adapter)
                .with_computer_config(computer_config);
        }
        if let Some(pa) = perception_adapter.clone() {
            builder = builder.with_perception_adapter(pa);
        }
        // Attach planner state store for crash recovery on restart.
        let planner_db = crate::dirs::syscity_dir().join("planner.db");
        let url = format!("sqlite:///{}", planner_db.display());
        if let Ok(store) = crate::planner::TaskStateStore::new(&url).await {
            builder = builder.with_planner_state_store(store);
        }
        Arc::new(builder)
    } else {
        let mut builder = Agent::new(config.clone(), provider, tools)
            .with_model(model.clone())
            .with_cost_guard(cost_guard)
            .with_skill_manager(Arc::clone(&state.tools.skills_manager))
            .with_transcript_store(Arc::clone(&state.infra.transcript_store))
            .with_artifact_store(Arc::clone(&state.infra.artifact_store))
            .with_disk_budget(Arc::clone(&state.infra.disk_budget))
            .with_session_file_manager(Arc::clone(&state.infra.session_file_manager))
            .with_model_router(Arc::clone(&state.infra.model_router))
            .with_model_alias(model.clone());
        if let Some(adapter) = computer_adapter.clone() {
            builder = builder
                .with_computer_adapter(adapter)
                .with_computer_config(computer_config);
        }
        if let Some(pa) = perception_adapter.clone() {
            builder = builder.with_perception_adapter(pa);
        }
        // Attach planner state store for crash recovery on restart.
        let planner_db = crate::dirs::syscity_dir().join("planner.db");
        let url = format!("sqlite:///{}", planner_db.display());
        if let Ok(store) = crate::planner::TaskStateStore::new(&url).await {
            builder = builder.with_planner_state_store(store);
        }
        Arc::new(builder)
    };

    // Wire the new agent into the cron scheduler so routine (agent-target)
    // jobs can run. Only the first agent is wired; subsequent agents keep
    // the first one active unless explicitly overwritten.
    {
        if let Some(cron_arc) = state.scheduler.cron_scheduler.read().await.clone() {
            cron_arc.lock().await.set_agent(agent.clone()).await;
            debug!("Routine engine: wired agent '{}' into cron scheduler", id);
        }
    }

    let (query_tx, query_rx) = tokio::sync::mpsc::channel::<super::AgentQuery>(32);

    let handle = super::AgentHandle {
        id: id.clone(),
        config: config.clone(),
        tx: tx.clone(),
        query_tx: query_tx.clone(),
        busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        agent: agent.clone(),
    };
    let handle_for_loop = handle.clone();

    {
        let mut agents = state.agents.agents.write().await;
        if agents.contains_key(&id) {
            return Err(crate::SyscityError::Validation(format!(
                "Agent '{}' is already running",
                id
            )));
        }
        agents.insert(id.clone(), handle);
    }

    // Register the per-agent stale-context eviction loop (check every 5 min,
    // evict contexts idle > 30 min) in the task registry so shutdown and agent
    // replacement can abort it uniformly.
    let task_registry = state.task_registry.clone();
    let repair_handle = agent.start_self_repair_loop(
        std::time::Duration::from_secs(300),
        std::time::Duration::from_secs(1800),
    );
    task_registry
        .insert_join(format!("agent:{}:repair", id), repair_handle)
        .await;

    // Start agent processing loop
    let agent_id = id.clone();
    let agent_for_loop = agent.clone();
    let busy = handle_for_loop.busy.clone();
    let state_for_loop = state.clone();

    let task_handle = tokio::spawn(async move {
        run_agent_loop(state_for_loop, agent_id, agent_for_loop, busy, rx, query_rx, true).await;
    });

    task_registry
        .insert_join(format!("agent:{}", id), task_handle)
        .await;

    Ok(())
}

// ── Shared agent processing loop ─────────────────────────────────────────────

/// Shared per-agent message/query processing loop.
///
/// `use_acp` selects between ACP-routed execution (production agents spawned by
/// the gateway) and direct agent execution (agents created via the REST API).
/// Broadcast a gateway event and log if there are no active listeners.
fn emit_event(
    tx: &tokio::sync::broadcast::Sender<super::GatewayEvent>,
    event: super::GatewayEvent,
) {
    if let Err(e) = tx.send(event) {
        warn!("Failed to broadcast gateway event: {}", e);
    }
}

pub(crate) async fn run_agent_loop(
    state: Arc<super::GatewayState>,
    agent_id: String,
    agent: Arc<Agent>,
    busy: Arc<std::sync::atomic::AtomicBool>,
    mut rx: tokio::sync::mpsc::Receiver<super::AgentCommand>,
    mut query_rx: tokio::sync::mpsc::Receiver<super::AgentQuery>,
    use_acp: bool,
) {
    use std::sync::atomic::Ordering;

    info!("Agent {} processing loop started", agent_id);

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let cmd = match cmd { Some(c) => c, None => break };
                match cmd {
                    super::AgentCommand::ProcessMessage {
                        session_id,
                        message,
                        user_id,
                        channel,
                        model_override,
                    } => {
                        busy.store(true, Ordering::Release);
                        emit_event(&state.events.tx, super::GatewayEvent::AgentStatus {
                            agent_id: agent_id.clone(),
                            status: super::AgentStatus::Processing {
                                session_id: session_id.clone(),
                            },
                        });

                        if use_acp {
                            process_message_acp(
                                &state,
                                &agent_id,
                                &agent,
                                &session_id,
                                &message,
                                &user_id,
                                &channel,
                            )
                            .await;
                        } else {
                            process_message_direct(
                                &state,
                                &agent_id,
                                &agent,
                                DirectMessage {
                                    session_id: session_id.clone(),
                                    message: message.clone(),
                                    user_id: user_id.clone(),
                                    channel: channel.clone(),
                                    model_override,
                                },
                            )
                            .await;
                        }

                        emit_event(&state.events.tx, super::GatewayEvent::AgentStatus {
                            agent_id: agent_id.clone(),
                            status: super::AgentStatus::Idle,
                        });
                        busy.store(false, Ordering::Release);
                    }
                    super::AgentCommand::Cancel => {
                        if use_acp {
                            warn!("Agent {} received cancel command (ACP path)", agent_id);
                        } else {
                            warn!("Agent {} received cancel command", agent_id);
                        }
                    }
                    super::AgentCommand::UpdateConfig(new_config) => {
                        info!("Agent {} updating configuration", agent_id);
                        {
                            let mut agents = state.agents.agents.write().await;
                            if let Some(handle) = agents.get_mut(&agent_id) {
                                handle.config = new_config.clone();
                                info!("Agent {} configuration updated", agent_id);
                            }
                        }
                        emit_event(&state.events.tx, super::GatewayEvent::AgentStatus {
                            agent_id: agent_id.clone(),
                            status: super::AgentStatus::Idle,
                        });
                    }
                    super::AgentCommand::Shutdown => {
                        info!("Agent {} shutting down", agent_id);
                        emit_event(&state.events.tx, super::GatewayEvent::AgentStatus {
                            agent_id: agent_id.clone(),
                            status: super::AgentStatus::Shutdown,
                        });
                        break;
                    }
                }
            }
            query = query_rx.recv() => {
                let query = match query { Some(q) => q, None => break };
                match query {
                    super::AgentQuery::GetThreadSummaries { response_tx } => {
                        if let Err(e) = response_tx.send(agent.thread_summaries().await) {
                            warn!("Agent {}: failed to send thread summaries: {:?}", agent_id, e);
                        }
                    }
                    super::AgentQuery::GetThreadTurns { conv_id, response_tx } => {
                        if let Err(e) = response_tx.send(agent.thread_turns_for(&conv_id).await) {
                            warn!(
                                "Agent {}: failed to send thread turns for {}: {:?}",
                                agent_id, conv_id, e
                            );
                        }
                    }
                    super::AgentQuery::UndoLastTurn { conv_id, response_tx } => {
                        if let Err(e) = response_tx.send(agent.undo_last_turn(&conv_id).await) {
                            warn!(
                                "Agent {}: failed to send undo result for {}: {:?}",
                                agent_id, conv_id, e
                            );
                        }
                    }
                    super::AgentQuery::RedoLastTurn { conv_id, response_tx } => {
                        if let Err(e) = response_tx.send(agent.redo_last_turn(&conv_id).await) {
                            warn!(
                                "Agent {}: failed to send redo result for {}: {:?}",
                                agent_id, conv_id, e
                            );
                        }
                    }
                    super::AgentQuery::RunSkill {
                        session_id,
                        message,
                        user_id,
                        skill_trust,
                        response_tx,
                    } => {
                        agent.set_skill_trust(skill_trust);
                        let incoming = crate::channels::IncomingMessage::new(
                            user_id,
                            &session_id,
                            message,
                        );
                        let no_op: crate::agent::ProgressCallback =
                            Arc::new(|_| Box::pin(async {}));
                        let result =
                            agent.process_message_with_progress(incoming, no_op).await;
                        agent.set_skill_trust(crate::tools::SkillTrust::Trusted);
                        if let Err(e) = response_tx.send(result) {
                            warn!(
                                "Agent {}: failed to send skill run result for {}: {:?}",
                                agent_id, session_id, e
                            );
                        }
                    }
                }
            }
        }
    }

    info!("Agent {} processing loop ended", agent_id);
}

/// Process a message through the ACP control plane.
async fn process_message_acp(
    state: &Arc<super::GatewayState>,
    agent_id: &str,
    agent: &Arc<Agent>,
    session_id: &str,
    message: &str,
    user_id: &str,
    channel: &str,
) {
    let incoming_msg = crate::channels::IncomingMessage::new(
        user_id.to_string(),
        session_id.to_string(),
        message.to_string(),
    )
    .with_provenance(crate::channels::InputProvenance::ExternalUser {
        channel: channel.to_string(),
        is_direct: true,
    });

    let trajectory = Arc::new(tokio::sync::Mutex::new(crate::outbound::TrajectoryLog::new()));
    {
        let mut traj = trajectory.lock().await;
        traj.push(crate::outbound::TrajectoryEntry::Start {
            timestamp: std::time::SystemTime::now(),
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
        });
    }

    let progress_state = state.clone();
    let progress_session_id = session_id.to_string();
    let progress_agent_id = agent_id.to_string();
    let progress_trajectory = trajectory.clone();
    let progress_cb: crate::agent::ProgressCallback = Arc::new(move |event| {
        let state = progress_state.clone();
        let session_id = progress_session_id.clone();
        let agent_id = progress_agent_id.clone();
        let trajectory = progress_trajectory.clone();
        Box::pin(async move {
            match event {
                crate::agent::ProgressEvent::ToolCalling { name, arguments } => {
                    info!("ToolCalling event: {} for session {}", name, session_id);
                    emit_event(
                        &state.events.tx,
                        super::GatewayEvent::ToolCalling {
                            session_id: session_id.clone(),
                            agent_id: agent_id.clone(),
                            tool_name: name.clone(),
                            arguments: arguments.clone(),
                        },
                    );
                    let mut traj = trajectory.lock().await;
                    traj.push(crate::outbound::TrajectoryEntry::ToolCall {
                        timestamp: std::time::SystemTime::now(),
                        name: name.clone(),
                        arguments: serde_json::from_str(&arguments)
                            .unwrap_or(serde_json::Value::String(arguments)),
                    });
                }
                crate::agent::ProgressEvent::ToolResult { name, result, data } => {
                    info!("ToolResult event: {} for session {}", name, session_id);
                    emit_event(
                        &state.events.tx,
                        super::GatewayEvent::ToolResult {
                            session_id: session_id.clone(),
                            agent_id: agent_id.clone(),
                            tool_name: name.clone(),
                            result: result.clone(),
                            data: data.clone(),
                        },
                    );
                    let mut traj = trajectory.lock().await;
                    traj.push(crate::outbound::TrajectoryEntry::ToolResult {
                        timestamp: std::time::SystemTime::now(),
                        name: name.clone(),
                        result: serde_json::from_str(&result)
                            .unwrap_or(serde_json::Value::String(result)),
                        duration_ms: 0,
                    });
                }
                crate::agent::ProgressEvent::Completed { response } => {
                    emit_event(
                        &state.events.tx,
                        super::GatewayEvent::Completed {
                            session_id: session_id.clone(),
                            agent_id: agent_id.clone(),
                            response,
                        },
                    );
                }
                crate::agent::ProgressEvent::Error { message } => {
                    emit_event(
                        &state.events.tx,
                        super::GatewayEvent::ProcessingError {
                            session_id: session_id.clone(),
                            agent_id: agent_id.clone(),
                            message,
                        },
                    );
                }
                _ => {}
            }
        })
    });

    let (response_content, response_usage) = match agent
        .process_message_with_progress(incoming_msg, progress_cb)
        .await
    {
        Ok(outgoing) => {
            let mut traj = trajectory.lock().await;
            traj.push(crate::outbound::TrajectoryEntry::Finish {
                timestamp: std::time::SystemTime::now(),
                output: outgoing.content.clone(),
            });
            (outgoing.content, outgoing.usage)
        }
        Err(e) => {
            error!("Agent {} failed to process message: {}", agent_id, e);
            let mut traj = trajectory.lock().await;
            traj.push(crate::outbound::TrajectoryEntry::Error {
                timestamp: std::time::SystemTime::now(),
                message: e.to_string(),
            });
            (format!("Error processing message: {}", e), None)
        }
    };

    let conversation_id = {
        let sessions = state.channels.session_channels.read().await;
        sessions
            .get(session_id)
            .map(|(_, cid)| cid.clone())
            .unwrap_or_else(|| session_id.to_string())
    };

    let run_id = uuid::Uuid::new_v4().to_string();

    if let Some(ref store) = state.agents.store {
        if let Err(e) = store
            .append_message(&AppendMessageParams {
                session_id,
                role: "assistant",
                content: &response_content,
                transcript_id: Some(session_id),
                run_id: Some(&run_id),
                ..Default::default()
            })
            .await
        {
            warn!("Failed to save assistant message to session history: {}", e);
        }
    }

    info!(
        "DEBUG: Agent {} sending AgentResponse for session {} (conversation: {})",
        agent_id, session_id, conversation_id
    );
    emit_event(
        &state.events.tx,
        super::GatewayEvent::AgentResponse {
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            content: response_content.clone(),
            channel: channel.to_string(),
            conversation_id: conversation_id.clone(),
            usage: response_usage,
        },
    );

    let trajectory = {
        let traj = trajectory.lock().await;
        traj.clone()
    };

    let outbound_ctx = crate::outbound::OutboundContext {
        session_id: session_id.to_string(),
        channel: channel.to_string(),
        agent_id: agent_id.to_string(),
        raw_output: response_content,
        tool_calls: vec![],
        trajectory,
        usage: response_usage,
    };
    let outbound_result = state.pipelines.outbound.process(outbound_ctx).await;

    if let Some(canvas_update) = outbound_result.canvas_update {
        state
            .tools
            .canvas_manager
            .apply_update(session_id, canvas_update)
            .await;
    }
}

/// Message payload for direct (non-ACP) agent execution.
struct DirectMessage {
    /// Target session ID.
    session_id: String,
    /// Message content.
    message: String,
    /// Originating user ID.
    user_id: String,
    /// Originating channel.
    channel: String,
    /// Optional model override.
    model_override: Option<String>,
}

/// Process a message directly (no ACP serialization).
async fn process_message_direct(
    state: &Arc<super::GatewayState>,
    agent_id: &str,
    agent: &Arc<Agent>,
    msg: DirectMessage,
) {
    use crate::agent::session_store::AppendMessageParams;

    let DirectMessage {
        ref session_id,
        ref message,
        ref user_id,
        ref channel,
        model_override,
    } = msg;

    let (reasoning_vis, verbose_mode) = {
        let settings = state.infra.runtime_settings.read().await;
        (
            settings
                .get("reasoning.visibility")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            settings
                .get("verbose.mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        )
    };

    let event_tx = state.events.tx.clone();
    let progress_session = session_id.to_string();
    let progress_agent = agent_id.to_string();
    let progress_cb: crate::agent::ProgressCallback = Arc::new(move |event| {
        let tx = event_tx.clone();
        let reasoning_vis = reasoning_vis.clone();
        let verbose_mode = verbose_mode.clone();
        let sid = progress_session.clone();
        let aid = progress_agent.clone();
        Box::pin(async move {
            match event {
                crate::agent::ProgressEvent::Started => {
                    emit_event(
                        &tx,
                        super::GatewayEvent::AgentStatus {
                            agent_id: aid.clone(),
                            status: super::AgentStatus::Processing { session_id: sid.clone() },
                        },
                    );
                }
                crate::agent::ProgressEvent::Generating { content } => {
                    if reasoning_vis.as_deref() == Some("off") {
                        return;
                    }
                    if let Some(ref thinking) = content {
                        if !thinking.is_empty() {
                            emit_event(
                                &tx,
                                super::GatewayEvent::Thinking {
                                    session_id: sid.clone(),
                                    agent_id: aid.clone(),
                                    content: Some(thinking.clone()),
                                },
                            );
                        }
                    }
                }
                crate::agent::ProgressEvent::ContentDelta { text } => {
                    emit_event(
                        &tx,
                        super::GatewayEvent::ContentDelta {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            delta: text,
                        },
                    );
                }
                crate::agent::ProgressEvent::ToolCalling { name, arguments } => {
                    if verbose_mode.as_deref() == Some("off") {
                        return;
                    }
                    emit_event(
                        &tx,
                        super::GatewayEvent::ToolCalling {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            tool_name: name.clone(),
                            arguments: arguments.clone(),
                        },
                    );
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
                    emit_event(
                        &tx,
                        super::GatewayEvent::ToolResult {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            tool_name: name.clone(),
                            result,
                            data,
                        },
                    );
                }
                crate::agent::ProgressEvent::ToolResultDelta { .. } => {}
                crate::agent::ProgressEvent::Completed { response } => {
                    emit_event(
                        &tx,
                        super::GatewayEvent::Completed {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            response,
                        },
                    );
                }
                crate::agent::ProgressEvent::Error { message } => {
                    emit_event(
                        &tx,
                        super::GatewayEvent::ProcessingError {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            message,
                        },
                    );
                }
            }
        })
    });

    let think_level = {
        let s = state.infra.runtime_settings.read().await;
        s.get("think.level")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
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

    agent.set_model_override(model_override).await;
    agent.set_extra_params(extra).await;

    let incoming_msg = crate::channels::IncomingMessage::new(
        user_id.to_string(),
        session_id.to_string(),
        message.to_string(),
    );

    let result = agent
        .process_message_with_progress(incoming_msg, progress_cb)
        .await;
    agent.set_model_override(None).await;

    match result {
        Ok(mut outgoing) => {
            let reasoning_vis = {
                let s = state.infra.runtime_settings.read().await;
                s.get("reasoning.visibility")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            };
            if reasoning_vis.as_deref() == Some("off") {
                outgoing.reasoning_content = None;
            }

            if let Some(ref usage) = outgoing.usage {
                let mut settings = state.infra.runtime_settings.write().await;
                let current_tokens = settings
                    .get("usage.tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let total_tokens = usage.prompt_tokens as u64 + usage.completion_tokens as u64;
                settings.insert(
                    "usage.tokens".to_string(),
                    serde_json::json!(current_tokens + total_tokens),
                );
                let current_calls = settings
                    .get("usage.calls")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let tool_calls = outgoing
                    .tool_calls
                    .as_ref()
                    .map(|c| c.len() as u64)
                    .unwrap_or(0);
                settings.insert(
                    "usage.calls".to_string(),
                    serde_json::json!(current_calls + tool_calls + 1),
                );
            }

            let run_id = uuid::Uuid::new_v4().to_string();
            if let Some(ref store) = state.agents.store {
                if let Err(e) = store
                    .append_message(&AppendMessageParams {
                        session_id,
                        role: "assistant",
                        content: &outgoing.content,
                        transcript_id: Some(session_id),
                        run_id: Some(&run_id),
                        ..Default::default()
                    })
                    .await
                {
                    warn!("Failed to save assistant message to session history: {}", e);
                }
            }

            emit_event(
                &state.events.tx,
                super::GatewayEvent::AgentResponse {
                    session_id: session_id.to_string(),
                    agent_id: agent_id.to_string(),
                    content: outgoing.content,
                    channel: channel.to_string(),
                    conversation_id: session_id.to_string(),
                    usage: outgoing.usage,
                },
            );
        }
        Err(e) => {
            error!("Agent {} failed to process: {}", agent_id, e);
        }
    }
}

// ── create_default_tool_registry ─────────────────────────────────────────────

/// Dependencies required to build the default tool registry.
pub(crate) struct ToolRegistryArgs {
    /// ACP control plane for subagent/session tools.
    pub acp: Arc<AcpControlPlane>,
    /// MCP manager for MCP-backed tools.
    pub mcp_manager: Arc<McpManager>,
    /// Shared approval queue.
    pub approval_queue: Arc<ApprovalQueue>,
    /// Persistent session store.
    pub session_store: Option<Arc<crate::agent::session_store::SessionStore>>,
    /// Lazy memory manager holder.
    pub memory_manager: Arc<RwLock<Option<Arc<crate::memory::MemoryManager>>>>,
    /// Capability flags.
    pub capabilities: CapabilitiesConfig,
    /// Audit logger.
    pub audit_log: Arc<dyn crate::security::runtime_audit::AuditLogger>,
    /// Optional content filter.
    pub content_filter: Option<Arc<crate::security::content_filter::ContentFilter>>,
}

/// Create default tool registry with all built-in tools
pub(crate) async fn create_default_tool_registry(
    args: ToolRegistryArgs,
) -> crate::Result<ToolRegistry> {
    use crate::tools::*;

    let ToolRegistryArgs {
        acp,
        mcp_manager,
        approval_queue,
        session_store,
        memory_manager,
        capabilities,
        audit_log,
        content_filter,
    } = args;

    let mut registry = ToolRegistry::new()
        .with_approval_queue(approval_queue)
        .with_audit_log(audit_log);
    if let Some(filter) = content_filter {
        registry = registry.with_content_filter(filter);
    }

    // Register file system tools
    registry.register(Box::new(FileReadTool::new()));
    registry.register(Box::new(FileWriteTool::new()));
    registry.register(Box::new(FileEditTool::new()));
    registry.register(Box::new(GlobTool::new()));
    registry.register(Box::new(GrepTool::new()));

    // Register shell/execution tools wrapped in sandbox for path & timeout
    // enforcement. ShellTool needs network access (git, curl, etc.);
    // CodeExecutionTool does not.
    registry.register(Box::new(SandboxedTool::new(
        ShellTool::new(),
        SandboxConfig {
            allow_network_access: true,
            ..SandboxConfig::default()
        },
    )));
    registry.register(Box::new(SandboxedTool::new(
        CodeExecutionTool::default(),
        SandboxConfig::default(),
    )));

    // Register web tools
    registry.register(Box::new(WebSearchTool::new()));
    registry.register(Box::new(WebFetchTool::new()));

    // Register todo tool
    registry.register(Box::new(TodoTool::new()));

    // Register cron tool
    registry.register(Box::new(CronTool::new()));

    // Register heartbeat tool (agent self-management)
    registry.register(Box::new(HeartbeatTool::new()));

    // Register time tool
    registry.register(Box::new(TimeTool::new()));

    // Register browser tool (if browser feature enabled)
    #[cfg(feature = "browser")]
    registry.register(Box::new(BrowserTool::new()));

    // Register ACP tools for subagent spawning
    registry.register(Box::new(AcpSpawnTool::new(acp.clone(), session_store.clone())));
    registry.register(Box::new(AcpSessionTool::new(acp.clone())));

    // Register session tools
    registry.register(Box::new(SessionsListTool::new(session_store.clone())));
    registry.register(Box::new(SessionsHistoryTool::new(session_store.clone())));
    registry.register(Box::new(SessionsSendTool::new(acp.clone())));
    registry.register(Box::new(SessionsYieldTool::new(acp.clone())));
    registry.register(Box::new(SessionStatusTool::new(session_store.clone())));
    registry.register(Box::new(ApplyPatchTool::new()));

    // Register memory tool for persistent memory storage
    match MemoryTool::new().await {
        Ok(memory_tool) => {
            registry.register(Box::new(memory_tool));
            info!("MemoryTool registered successfully");
        }
        Err(e) => {
            warn!(
                "Failed to initialize MemoryTool: {}. Memory functionality will not be available.",
                e
            );
        }
    }

    // Register semantic/hybrid memory search tool
    match MemorySearchTool::new().await {
        Ok(tool) => {
            let tool = tool.with_manager_holder(memory_manager);
            registry.register(Box::new(tool));
            info!("MemorySearchTool registered successfully");
        }
        Err(e) => {
            warn!("Failed to initialize MemorySearchTool: {}. Hybrid search unavailable.", e);
        }
    }

    // Register memory get/CRUD tool
    match MemoryGetTool::new().await {
        Ok(tool) => {
            registry.register(Box::new(tool));
            info!("MemoryGetTool registered successfully");
        }
        Err(e) => {
            warn!("Failed to initialize MemoryGetTool: {}. Memory CRUD unavailable.", e);
        }
    }

    // Register MCP (Model Context Protocol) connection tool (uses shared manager)
    registry.register(Box::new(McpConnectionTool::with_manager(mcp_manager)));

    // Register plan management tool
    registry.register(Box::new(UpdatePlanTool::new()));

    // Register process management tool
    registry.register(Box::new(ProcessTool::new()));

    // Register PDF generation tool
    registry.register(Box::new(PdfTool::new()));

    // Register image tools
    registry.register(Box::new(ImageTool::new()));
    registry.register(Box::new(ImageGenerateTool::new()));

    // Register TTS tool
    registry.register(Box::new(TtsTool::new()));

    // Register STT tool
    registry.register(Box::new(SttTool::new()));

    // Register nodes/Tailscale tool
    registry.register(Box::new(NodesTool::new()));

    // Register capability discovery tool
    registry.register(Box::new(ListCapabilitiesTool::new()));

    // ── Register platform-specific capability sets ──
    {
        use crate::computer::platform::{
            CapabilityProfile, OsControlScope, PlatformCapabilityRegistry, ToolConflictStrategy,
        };

        let mut tool_reg = PlatformCapabilityRegistry::new();

        #[cfg(target_os = "linux")]
        {
            tool_reg.register(Box::new(crate::computer::platform::LinuxToolset::new()));
            tool_reg.register(Box::new(crate::computer::platform::LinuxDesktopX11Toolset::new()));
            tool_reg
                .register(Box::new(crate::computer::platform::LinuxDesktopWaylandToolset::new()));
        }

        #[cfg(target_os = "macos")]
        {
            tool_reg.register(Box::new(crate::computer::platform::MacosToolset::new()));
        }

        #[cfg(target_os = "windows")]
        {
            tool_reg.register(Box::new(crate::computer::platform::WindowsToolset::new()));
        }

        // Load capability profile from config
        let profile = match capabilities.profile.as_str() {
            "minimal" => CapabilityProfile::Minimal,
            "observer" => CapabilityProfile::Observer,
            "server" => CapabilityProfile::Server,
            "desktop" => CapabilityProfile::Desktop,
            "custom" => CapabilityProfile::Custom(capabilities.custom_sets.clone()),
            _ => CapabilityProfile::Full,
        };
        let max_scope = match capabilities.max_scope.as_str() {
            "read_only" => Some(OsControlScope::ReadOnly),
            "user_space" => Some(OsControlScope::UserSpace),
            "system" => Some(OsControlScope::System),
            "root" => Some(OsControlScope::Root),
            _ => None,
        };
        let disabled_sets: std::collections::HashSet<String> =
            capabilities.disabled_sets.iter().cloned().collect();

        profile.apply(&mut tool_reg);

        // Apply max_scope filter: disable sets whose scope exceeds the limit
        if let Some(limit) = max_scope {
            let to_disable: Vec<String> = tool_reg
                .all_sets()
                .iter()
                .filter(|s| s.scope() > limit)
                .map(|s| s.id().to_string())
                .collect();
            for id in to_disable {
                tool_reg.disable(&id);
            }
        }

        // Apply explicit disabled_sets filter
        for id in &disabled_sets {
            tool_reg.disable(id);
        }

        // Log detected capabilities before exporting
        let available = tool_reg.available_sets();
        if available.is_empty() {
            info!("No platform-specific tool sets detected on this host");
        } else {
            for set in &available {
                info!(
                    "Platform tool set available: {} ({}) — {}",
                    set.name(),
                    set.id(),
                    set.description()
                );
            }
        }

        tool_reg.export_to_tool_registry(&mut registry, ToolConflictStrategy::Reject);

        info!("Platform tool sets exported: {} set(s) active", available.len());
    }

    // Gate high-privilege tools behind SkillTrust::Trusted.
    // Community-trust skills see only read-only / informational tools.
    registry.mark_privileged("shell");
    registry.mark_privileged("execute_code");
    registry.mark_privileged("file_write");
    registry.mark_privileged("file_edit");
    registry.mark_privileged("delegate");
    registry.mark_privileged("acp_spawn");
    registry.mark_privileged("acp_session");
    registry.mark_privileged("memory");
    registry.mark_privileged("sessions_send");
    registry.mark_privileged("sessions_yield");
    registry.mark_privileged("subagents");
    registry.mark_privileged("apply_patch");
    registry.mark_privileged("message");
    registry.mark_privileged("process");
    registry.mark_privileged("image_generate");

    // OS control tools — privileged because they modify system state.
    registry.mark_privileged("system_inspect");
    registry.mark_privileged("service_manager");

    Ok(registry)
}

// ── build_summarizer ─────────────────────────────────────────────────────────

/// Build the summarizer backend selected by configuration.
///
/// * `Template` — always available, zero-LLM, rule-based.
/// * `Llm` — uses the agent's LLM provider (same cost as a normal model call).
/// * `Local` — requires the `local-summarizer` feature (Qwen2.5-1.5B GGUF). If
///   the feature is missing or model loading fails, falls back to `Template`
///   with a warning so agent spawn never panics.
async fn build_summarizer(
    kind: &super::config::SummarizerKind,
    provider: Arc<dyn crate::providers::Provider>,
    model: String,
) -> Arc<dyn crate::perception::PerceptionSummarizer> {
    match kind {
        super::config::SummarizerKind::Template => {
            Arc::new(crate::perception::TemplateSummarizer::new())
        }
        super::config::SummarizerKind::Llm => {
            Arc::new(crate::perception::LlmProviderSummarizer::new(provider).with_model(model))
        }
        super::config::SummarizerKind::Local => {
            #[cfg(feature = "local-summarizer")]
            {
                match crate::perception::local_summarizer::LocalLlamaSummarizer::new_auto().await {
                    Ok(s) => return Arc::new(s),
                    Err(e) => {
                        tracing::warn!(
                            "Local summarizer init failed: {e}; falling back to TemplateSummarizer"
                        );
                    }
                }
            }
            #[cfg(not(feature = "local-summarizer"))]
            {
                tracing::warn!(
                    "summarizer_kind = \"local\" but feature local-summarizer is not enabled; \
                     falling back to TemplateSummarizer"
                );
            }
            Arc::new(crate::perception::TemplateSummarizer::new())
        }
    }
}
