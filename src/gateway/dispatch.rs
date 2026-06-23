//! Inbound message dispatch — workers, queue-mode handling, agent fan-out.
//!
//! Extracted from `gateway/mod.rs`. Provides the unified inbound entry worker,
//! the routed-message worker, and the per-message dispatch logic that fans
//! `RoutedMessage` into the resolved agent's ACP queue.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::{AgentCommand, AgentStatus, BufferedMessage, GatewayEvent, GatewayState};
use crate::agent::session_store::AppendMessageParams;

/// Unified worker that consumes `IncomingMessage`s from `inbound_entry`
/// and drives them through the inbound pipeline.
///
/// The pipeline forwards `RoutedMessage`s to `routed_tx`; the separate
/// `process_routed_messages` worker handles actual agent dispatch.
pub(crate) async fn process_inbound_entries(
    state: Arc<GatewayState>,
    mut rx: mpsc::Receiver<crate::channels::IncomingMessage>,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("Inbound entry worker received shutdown signal");
                break;
            }
            Some(incoming) = rx.recv() => {
                match state.pipelines.inbound.process(incoming).await {
                    Some(routed) => {
                        info!("Inbound message routed through pipeline: agent={}", routed.agent_id);
                    }
                    None => {
                        info!("Inbound message absorbed by pipeline (debounced or suppressed)");
                    }
                }
            }
        }
    }

    // Drain any in-flight messages with a bounded timeout.
    let drain_deadline = Duration::from_secs(5);
    while let Ok(Some(incoming)) = timeout(drain_deadline, rx.recv()).await {
        let _ = state.pipelines.inbound.process(incoming).await;
    }
    info!("Inbound entry worker stopped");
}

/// Process routed messages from the inbound pipeline.
///
/// Converts `RoutedMessage` into `AgentCommand::ProcessMessage` and
/// forwards it to the resolved agent, respecting `QueueMode`.
pub(crate) async fn process_routed_messages(
    state: Arc<GatewayState>,
    mut rx: mpsc::Receiver<crate::inbound::RoutedMessage>,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("Routed message worker received shutdown signal");
                break;
            }
            Some(routed) = rx.recv() => {
                dispatch_routed_message(&state, routed).await;
            }
        }
    }

    // Drain any in-flight routed messages with a bounded timeout.
    let drain_deadline = Duration::from_secs(5);
    while let Ok(Some(routed)) = timeout(drain_deadline, rx.recv()).await {
        dispatch_routed_message(&state, routed).await;
    }
    info!("Routed message worker stopped");
}

/// Dispatch a single `RoutedMessage` to the resolved agent.
///
/// Lock hierarchy observed in this function:
/// `agents.group_manager` → `agents.message_buffer` → `agents.follow_up_timers`
/// → `agents.agents` → `infra.runtime_settings`.
pub(crate) async fn dispatch_routed_message(
    state: &Arc<GatewayState>,
    routed: crate::inbound::RoutedMessage,
) {
    if routed.suppress_delivery {
        debug!("Suppressing delivery for session {}", routed.incoming.conversation_id.0);
        return;
    }

    let session_id = routed.incoming.conversation_id.0.clone();
    let agent_id = routed.agent_id.clone();
    let channel = match &routed.incoming.provenance {
        crate::channels::InputProvenance::ExternalUser { channel, .. } => channel.clone(),
        _ => "unknown".to_string(),
    };

    // ── Group session membership check ───────────────────────────────
    {
        let user_id = &routed.incoming.user_id.0;
        let groups = state.agents.group_manager.read().await;
        if let Some(group) = groups.get_group(&session_id) {
            let group = group.read().await;
            if !group.is_member(user_id) {
                warn!(
                    "User {} is not a member of group session {}, dropping message",
                    user_id, session_id
                );
                return;
            }
            if let Some(member) = group.get_member(user_id) {
                if !member.role.can_participate() {
                    warn!(
                        "User {} (role: {}) cannot participate in group session {}, dropping \
                         message",
                        user_id, member.role, session_id
                    );
                    return;
                }
            }
        }
    }

    // ── Cache runtime settings used for this dispatch ─────────────────
    let (think_level, queue_mode) = {
        let settings = state.infra.runtime_settings.read().await;
        (
            settings
                .get("think.level")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            settings
                .get("queue.mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        )
    };

    match routed.queue_mode {
        crate::inbound::QueueMode::Interrupt => {
            // Clear any buffered messages and pending timer for this session
            {
                let mut buffers = state.agents.message_buffer.write().await;
                buffers.remove(&session_id);
            }
            clear_follow_up_timer(state, &session_id).await;
            send_to_agent(
                state,
                AgentDispatch {
                    agent_id: agent_id.clone(),
                    session_id: session_id.clone(),
                    message: routed.incoming.content.clone(),
                    user_id: routed.incoming.user_id.0.clone(),
                    channel: channel.clone(),
                    think_level: think_level.clone(),
                    queue_mode: queue_mode.clone(),
                },
            )
            .await;
        }

        crate::inbound::QueueMode::Steer => {
            // Send Cancel to agent (best-effort), then send the steer message
            {
                let agents = state.agents.agents.read().await;
                if let Some(agent) = agents.get(&agent_id) {
                    if let Err(e) = agent.tx.send(AgentCommand::Cancel).await {
                        warn!("Failed to send cancel to agent {}: {}", agent_id, e);
                    }
                }
            }
            clear_follow_up_timer(state, &session_id).await;
            // Small delay to let cancel take effect
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            send_to_agent(
                state,
                AgentDispatch {
                    agent_id: agent_id.clone(),
                    session_id: session_id.clone(),
                    message: routed.incoming.content.clone(),
                    user_id: routed.incoming.user_id.0.clone(),
                    channel: channel.clone(),
                    think_level: think_level.clone(),
                    queue_mode: queue_mode.clone(),
                },
            )
            .await;
        }

        crate::inbound::QueueMode::FollowUp => {
            // Buffer message; flush after a delay if no more arrive.
            let should_flush = {
                let mut buffers = state.agents.message_buffer.write().await;
                let buffer = buffers.entry(session_id.clone()).or_default();
                buffer.push(BufferedMessage {
                    content: routed.incoming.content.clone(),
                    user_id: routed.incoming.user_id.0.clone(),
                    channel: channel.clone(),
                });
                buffer.len() >= 5 // Max 5 messages before forced flush
            };

            if should_flush {
                clear_follow_up_timer(state, &session_id).await;
                flush_session_buffer(
                    state,
                    &agent_id,
                    &session_id,
                    think_level.clone(),
                    queue_mode.clone(),
                )
                .await;
            } else {
                let mut timers = state.agents.follow_up_timers.write().await;
                if !timers.contains_key(&session_id) {
                    let state_clone = state.clone();
                    let agent_id_clone = agent_id.clone();
                    let session_id_clone = session_id.clone();
                    let think_level_clone = think_level.clone();
                    let queue_mode_clone = queue_mode.clone();
                    let handle = tokio::spawn(async move {
                        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                        flush_session_buffer(
                            &state_clone,
                            &agent_id_clone,
                            &session_id_clone,
                            think_level_clone,
                            queue_mode_clone,
                        )
                        .await;
                        state_clone
                            .agents
                            .follow_up_timers
                            .write()
                            .await
                            .remove(&session_id_clone);
                    });
                    timers.insert(session_id.clone(), handle);
                }
            }
        }

        crate::inbound::QueueMode::Collect => {
            // /done trigger: flush the buffer
            clear_follow_up_timer(state, &session_id).await;
            let has_buffered = {
                let buffers = state.agents.message_buffer.read().await;
                buffers
                    .get(&session_id)
                    .map(|b| !b.is_empty())
                    .unwrap_or(false)
            };

            if has_buffered {
                flush_session_buffer(
                    state,
                    &agent_id,
                    &session_id,
                    think_level.clone(),
                    queue_mode.clone(),
                )
                .await;
            } else {
                // No buffer to flush; treat as normal message
                send_to_agent(
                    state,
                    AgentDispatch {
                        agent_id: agent_id.clone(),
                        session_id: session_id.clone(),
                        message: routed.incoming.content.clone(),
                        user_id: routed.incoming.user_id.0.clone(),
                        channel: channel.clone(),
                        think_level: think_level.clone(),
                        queue_mode: queue_mode.clone(),
                    },
                )
                .await;
            }
        }

        crate::inbound::QueueMode::Normal => {
            send_to_agent(
                state,
                AgentDispatch {
                    agent_id: agent_id.clone(),
                    session_id: session_id.clone(),
                    message: routed.incoming.content.clone(),
                    user_id: routed.incoming.user_id.0.clone(),
                    channel: channel.clone(),
                    think_level: think_level.clone(),
                    queue_mode: queue_mode.clone(),
                },
            )
            .await;
        }
    }
}

/// Cancel any pending FollowUp flush timer for a session.
async fn clear_follow_up_timer(state: &GatewayState, session_id: &str) {
    if let Some(handle) = state
        .agents
        .follow_up_timers
        .write()
        .await
        .remove(session_id)
    {
        handle.abort();
    }
}

/// Flush buffered messages for a session and send as a single batch.
pub(crate) async fn flush_session_buffer(
    state: &Arc<GatewayState>,
    agent_id: &str,
    session_id: &str,
    think_level: Option<String>,
    queue_mode: Option<String>,
) {
    let messages: Vec<BufferedMessage> = {
        let mut buffers = state.agents.message_buffer.write().await;
        buffers.remove(session_id).unwrap_or_default()
    };

    if messages.is_empty() {
        return;
    }

    let combined = messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n\n");

    let first_user_id = messages
        .first()
        .map(|m| m.user_id.clone())
        .unwrap_or_default();
    let first_channel = messages
        .first()
        .map(|m| m.channel.clone())
        .unwrap_or_default();

    info!(
        "Flushing {} buffered messages for session {} (combined length: {})",
        messages.len(),
        session_id,
        combined.len()
    );

    send_to_agent(
        state,
        AgentDispatch {
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            message: combined,
            user_id: first_user_id,
            channel: first_channel,
            think_level,
            queue_mode,
        },
    )
    .await;
}

/// Extract a concise session name from the first assistant response.
/// Strips markdown, takes the first meaningful words, and limits length.
pub(crate) fn extract_session_name(content: &str) -> String {
    // Strip common markdown patterns
    let cleaned = content
        .replace("#", "")
        .replace("**", "")
        .replace("*", "")
        .replace("`", "")
        .replace(">", "")
        .replace("-", "")
        .replace("|", "")
        .replace("\n", " ")
        .replace("\r", " ");

    let name = cleaned
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");

    if name.len() > 40 {
        format!("{}...", &name[..40])
    } else if name.is_empty() {
        "New Session".to_string()
    } else {
        name
    }
}

/// Payload for dispatching a single message to an agent via ACP.
#[derive(Clone)]
pub(crate) struct AgentDispatch {
    /// Target agent ID.
    pub agent_id: String,
    /// Target session ID.
    pub session_id: String,
    /// Message content.
    pub message: String,
    /// Originating user ID.
    pub user_id: String,
    /// Originating channel.
    pub channel: String,
    /// Optional thinking level from runtime settings.
    pub think_level: Option<String>,
    /// Optional queue mode override.
    pub queue_mode: Option<String>,
}

/// Send a single message to an agent via the ACP controller.
///
/// This routes execution through the centralized ACP actor queue,
/// enabling per-session serial processing and runtime controls
/// (pause / resume / step / cancel).
pub(crate) async fn send_to_agent(state: &Arc<GatewayState>, dispatch: AgentDispatch) {
    let AgentDispatch {
        ref agent_id,
        ref session_id,
        ref message,
        ref user_id,
        ref channel,
        think_level,
        queue_mode,
    } = dispatch;

    // Pre-read directive settings so the progress callback does not need to
    // capture the runtime_settings Arc across await boundaries.
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

    let agents = state.agents.agents.read().await;
    let agent_handle = match agents.get(agent_id) {
        Some(h) => h.clone(),
        None => {
            error!("Agent {} not found for session {}", agent_id, session_id);
            return;
        }
    };
    drop(agents);

    // Apply thinking config from runtime settings
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
    agent_handle.agent.set_extra_params(extra).await;

    // Check queue mode and apply interrupt behavior if needed
    if queue_mode.as_deref() == Some("interrupt") {
        if let Err(e) = state.agents.acp.cancel(session_id.to_string()).await {
            warn!("Failed to cancel ACP session {}: {}", session_id, e);
        }
    }

    let incoming_msg = crate::channels::IncomingMessage::new(
        user_id.to_string(),
        session_id.to_string(),
        message.to_string(),
    )
    .with_provenance(crate::channels::InputProvenance::ExternalUser {
        channel: channel.to_string(),
        is_direct: true,
    });

    // Broadcast processing status
    if let Err(e) = state.events.tx.send(GatewayEvent::AgentStatus {
        agent_id: agent_id.to_string(),
        status: AgentStatus::Processing {
            session_id: session_id.to_string(),
        },
    }) {
        debug!("No receivers for AgentStatus event: {}", e);
    }

    // Build progress callback that forwards events to gateway subscribers
    let event_tx = state.events.tx.clone();
    let progress_session = session_id.to_string();
    let progress_agent = agent_id.to_string();
    let progress_channel = channel.to_string();
    let progress_cb: crate::agent::ProgressCallback = Arc::new(move |event| {
        let tx = event_tx.clone();
        let reasoning_vis = reasoning_vis.clone();
        let verbose_mode = verbose_mode.clone();
        let sid = progress_session.clone();
        let aid = progress_agent.clone();
        let _ch = progress_channel.clone();
        Box::pin(async move {
            match event {
                crate::agent::ProgressEvent::Started => {
                    if let Err(e) = tx.send(GatewayEvent::AgentStatus {
                        agent_id: aid.clone(),
                        status: AgentStatus::Processing { session_id: sid.clone() },
                    }) {
                        debug!("No receivers for AgentStatus event: {}", e);
                    }
                }
                crate::agent::ProgressEvent::Generating { content } => {
                    // Skip reasoning events if visibility is off
                    if reasoning_vis.as_deref() == Some("off") {
                        return;
                    }
                    // Only emit thinking events when there's actual content
                    if let Some(ref thinking) = content {
                        if !thinking.is_empty() {
                            if let Err(e) = tx.send(GatewayEvent::Thinking {
                                session_id: sid.clone(),
                                agent_id: aid.clone(),
                                content: Some(thinking.clone()),
                            }) {
                                debug!("No receivers for Thinking event: {}", e);
                            }
                        }
                    }
                }
                crate::agent::ProgressEvent::ContentDelta { text } => {
                    if let Err(e) = tx.send(GatewayEvent::ContentDelta {
                        session_id: sid.clone(),
                        agent_id: aid.clone(),
                        delta: text,
                    }) {
                        debug!("No receivers for ContentDelta event: {}", e);
                    }
                }
                crate::agent::ProgressEvent::ToolCalling { name, arguments } => {
                    // Skip tool events if verbose is off
                    if verbose_mode.as_deref() == Some("off") {
                        return;
                    }
                    if let Err(e) = tx.send(GatewayEvent::ToolCalling {
                        session_id: sid.clone(),
                        agent_id: aid.clone(),
                        tool_name: name.clone(),
                        arguments: arguments.clone(),
                    }) {
                        debug!("No receivers for ToolCalling event: {}", e);
                    }
                }
                crate::agent::ProgressEvent::ToolResult { name, result, data } => {
                    // Skip tool events if verbose is off
                    if verbose_mode.as_deref() == Some("off") {
                        return;
                    }
                    // In compact verbose mode, truncate long results
                    let result = if verbose_mode.as_deref() == Some("compact") {
                        if result.len() > 500 {
                            format!("{}... (truncated)", &result[..500])
                        } else {
                            result
                        }
                    } else {
                        result
                    };
                    if let Err(e) = tx.send(GatewayEvent::ToolResult {
                        session_id: sid.clone(),
                        agent_id: aid.clone(),
                        tool_name: name.clone(),
                        result,
                        data,
                    }) {
                        debug!("No receivers for ToolResult event: {}", e);
                    }
                }
                crate::agent::ProgressEvent::ToolResultDelta { .. } => {
                    // Streaming tool chunks are accumulated locally and emitted
                    // as a final ToolResult event; no per-chunk gateway event
                    // yet.
                }
                crate::agent::ProgressEvent::Completed { response } => {
                    if let Err(e) = tx.send(GatewayEvent::Completed {
                        session_id: sid.clone(),
                        agent_id: aid.clone(),
                        response,
                    }) {
                        debug!("No receivers for Completed event: {}", e);
                    }
                }
                crate::agent::ProgressEvent::Error { message } => {
                    if let Err(e) = tx.send(GatewayEvent::ProcessingError {
                        session_id: sid.clone(),
                        agent_id: aid.clone(),
                        message,
                    }) {
                        debug!("No receivers for ProcessingError event: {}", e);
                    }
                }
            }
        })
    });

    // Route through ACP for serialized execution
    match state
        .agents
        .acp
        .execute_session_with_progress(agent_handle.agent.clone(), incoming_msg, progress_cb)
        .await
    {
        Ok(mut outgoing) => {
            // Apply reasoning visibility filter
            let reasoning_vis = {
                let s = state.infra.runtime_settings.read().await;
                s.get("reasoning.visibility")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            };
            if reasoning_vis.as_deref() == Some("off") {
                outgoing.reasoning_content = None;
            }

            // Accumulate usage statistics (minimal write-lock hold)
            if let Some(ref usage) = outgoing.usage {
                {
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
            }

            // Generate run_id for this agent execution (run tracking)
            let run_id = uuid::Uuid::new_v4().to_string();

            // Save assistant response to persistent session history
            if let Some(ref store) = state.agents.store {
                let reasoning = outgoing.reasoning_content.as_deref();
                let tool_calls_json = outgoing
                    .tool_calls
                    .as_ref()
                    .map(|calls| serde_json::to_string(calls).unwrap_or_default());
                if let Err(e) = store
                    .append_message(&AppendMessageParams {
                        session_id,
                        role: "assistant",
                        content: &outgoing.content,
                        reasoning_content: reasoning,
                        tool_calls_json: tool_calls_json.as_deref(),
                        transcript_id: Some(session_id),
                        run_id: Some(&run_id),
                        ..Default::default()
                    })
                    .await
                {
                    warn!("Failed to save assistant message to session history: {}", e);
                }

                // Auto-name session from first assistant response if no name yet
                if let Ok(existing) = store.get_session_name(session_id).await {
                    if existing.is_none() {
                        let name = extract_session_name(&outgoing.content);
                        if let Err(e) = store.set_session_name(session_id, &name).await {
                            warn!("Failed to save session name: {}", e);
                        } else {
                            info!("Session {} auto-named: '{}'", session_id, name);
                            if let Err(e) = state.events.tx.send(GatewayEvent::SessionRenamed {
                                session_id: session_id.to_string(),
                                name: name.clone(),
                            }) {
                                debug!("No receivers for SessionRenamed event: {}", e);
                            }
                        }
                    }
                }
            }
            if let Err(e) = state.events.tx.send(GatewayEvent::AgentResponse {
                session_id: session_id.to_string(),
                agent_id: agent_id.to_string(),
                content: outgoing.content,
                channel: channel.to_string(),
                conversation_id: session_id.to_string(),
                usage: outgoing.usage,
            }) {
                debug!("No receivers for AgentResponse event: {}", e);
            }
        }
        Err(e) => {
            error!("ACP execution failed for agent {} session {}: {}", agent_id, session_id, e);
            if let Err(e) = state.events.tx.send(GatewayEvent::ProcessingError {
                session_id: session_id.to_string(),
                agent_id: agent_id.to_string(),
                message: format!("Execution failed: {}", e),
            }) {
                debug!("No receivers for ProcessingError event: {}", e);
            }
        }
    }

    if let Err(e) = state.events.tx.send(GatewayEvent::AgentStatus {
        agent_id: agent_id.to_string(),
        status: AgentStatus::Idle,
    }) {
        debug!("No receivers for AgentStatus event: {}", e);
    }
}
