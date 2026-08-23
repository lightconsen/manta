use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::AcpControlPlane;
use crate::acp::config::{SpawnMode, SubagentConfig, SubagentStatus, ThreadBinding, ThreadContext};
use crate::acp::controller::ExecutionController;
use crate::acp::session::AcpCommand;
use crate::acp::subagent::{SubagentCommand, SubagentHandle};
use crate::agent::session_store::SaveSubagentRunParams;

impl AcpControlPlane {
    /// Spawn a subagent
    pub async fn spawn_subagent(
        &self,
        session_id: crate::acp::config::AcpSessionId,
        parent_id: String,
        config: SubagentConfig,
    ) -> crate::Result<SubagentHandle> {
        let subagent_id = format!("subagent-{}", Uuid::new_v4());

        // Resolve thread ID (acquire guard, read, release)
        let thread_id = {
            let threads = self.threads.read().await;
            match &config.thread_binding {
                ThreadBinding::New => format!("thread-{}", Uuid::new_v4()),
                ThreadBinding::Parent => format!("thread-{}", parent_id),
                ThreadBinding::Thread(id) => id.clone(),
                ThreadBinding::Auto => {
                    // If the parent already has an associated thread, reuse it;
                    // otherwise create a fresh thread for this subagent.
                    let parent_thread_id = format!("thread-{}", parent_id);
                    if threads.contains_key(&parent_thread_id) {
                        threads
                            .get(&parent_thread_id)
                            .map(|t| t.id.clone())
                            .unwrap_or_else(|| parent_thread_id.clone())
                    } else {
                        format!("thread-{}", Uuid::new_v4())
                    }
                }
            }
        };

        info!(
            "Spawning subagent {} (mode: {:?}, thread: {})",
            subagent_id, config.mode, thread_id
        );

        // Create command channel
        let (command_tx, mut command_rx) = mpsc::channel::<SubagentCommand>(100);

        // Create the agent (acquire builder, call, release). The builder is
        // given the subagent id so the constructed agent is tagged with it.
        let agent = {
            let builder_guard = self.default_agent_builder.read().await;
            let result = if let Some(ref builder) = *builder_guard {
                builder(&subagent_id)
            } else {
                Err(crate::error::SyscityError::Internal("No agent builder configured".to_string()))
            };
            drop(builder_guard); // explicitly release before continuing
            result?
        };

        // Create execution controller for runtime pause/resume/step/cancel
        let controller = ExecutionController::new();
        let controller_clone = controller.clone();

        // Spawn subagent task
        let subagent_id_clone = subagent_id.clone();
        let mode = config.mode;
        let timeout = config.timeout_seconds;
        let max_iterations = self.max_iterations;

        // Capture fields needed for crash recovery.
        let acp_for_recovery = self.clone();
        let recovery_session_id = session_id.clone();
        let recovery_parent_id = parent_id.clone();
        let recovery_config_clone = config.clone();

        let join_handle = tokio::spawn(async move {
            info!("Subagent {} task started", subagent_id_clone);
            let agent = agent;

            while let Some(cmd) = command_rx.recv().await {
                match cmd {
                    SubagentCommand::ProcessMessage { message, response_tx } => {
                        let message = *message;
                        debug!("Subagent {} processing message", subagent_id_clone);

                        // Build a debug-logging callback so tool activity inside
                        // the subagent surfaces in logs.
                        let sid_cb = subagent_id_clone.clone();
                        let progress_cb: crate::agent::ProgressCallback = Arc::new(move |event| {
                            let sid = sid_cb.clone();
                            Box::pin(async move {
                                match event {
                                    crate::agent::ProgressEvent::ToolCalling {
                                        name,
                                        arguments,
                                    } => {
                                        debug!(
                                            "Subagent {} calling tool {}: {}",
                                            sid, name, arguments
                                        );
                                    }
                                    crate::agent::ProgressEvent::ToolResult {
                                        name,
                                        result,
                                        ..
                                    } => {
                                        debug!(
                                            "Subagent {} tool {} result: {} chars",
                                            sid,
                                            name,
                                            result.len()
                                        );
                                    }
                                    crate::agent::ProgressEvent::Error { message } => {
                                        warn!("Subagent {} progress error: {}", sid, message);
                                    }
                                    _ => {}
                                }
                            })
                        });

                        controller.reset().await;
                        let max_iter = max_iterations;

                        let result = tokio::time::timeout(
                            std::time::Duration::from_secs(timeout.unwrap_or(300)),
                            async {
                                if mode == SpawnMode::Run {
                                    agent
                                        .run_message_with_controller(
                                            message,
                                            controller.clone(),
                                            max_iter,
                                        )
                                        .await
                                } else {
                                    agent
                                        .process_message_with_progress_and_controller(
                                            message,
                                            progress_cb,
                                            controller.clone(),
                                            max_iter,
                                        )
                                        .await
                                }
                            },
                        )
                        .await;

                        controller.reset().await;

                        let response: crate::Result<String> = match result {
                            Ok(Ok(response)) => Ok(response.content),
                            Ok(Err(e)) => Err(e),
                            Err(_) => Err(crate::error::SyscityError::SubagentTimeout),
                        };

                        if let Err(e) = response_tx.send(response) {
                            warn!(
                                "Subagent {} failed to send response: {:?}",
                                subagent_id_clone, e
                            );
                        }

                        // For Run mode, terminate after first message
                        if mode == SpawnMode::Run {
                            info!("Subagent {} (Run mode) completing", subagent_id_clone);
                            break;
                        }
                    }
                    SubagentCommand::Cancel => {
                        debug!("Subagent {} cancelled", subagent_id_clone);
                        controller.cancel().await;
                    }
                    SubagentCommand::Shutdown => {
                        info!("Subagent {} shutting down", subagent_id_clone);
                        break;
                    }
                }
            }

            info!("Subagent {} task ended", subagent_id_clone);
        });

        let abort_handle = join_handle.abort_handle();

        // Watchdog: await the JoinHandle and update status on exit or panic.
        tokio::spawn(watchdog_task(
            join_handle,
            Arc::clone(&self.subagents),
            subagent_id.clone(),
            self.store.clone(),
            self.clone(),
            session_id.clone(),
            RecoveryPlan {
                acp: acp_for_recovery,
                session_id: recovery_session_id,
                parent_id: recovery_parent_id,
                config: recovery_config_clone,
            },
        ));

        let handle = SubagentHandle {
            id: subagent_id.clone(),
            parent_id: parent_id.clone(),
            session_id: session_id.clone(),
            mode: config.mode,
            thread_id: thread_id.clone(),
            command_tx,
            status: SubagentStatus::Ready,
            controller: controller_clone,
            abort_handle,
            crash_count: 0,
        };

        // Register subagent, session, and thread in a consistent lock order:
        // subagents -> sessions -> threads.
        let mut subagents = self.subagents.write().await;
        subagents.insert(subagent_id.clone(), handle.clone());

        {
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(&session_id) {
                session.subagents.push(subagent_id.clone());
                // Persist updated subagent list
                if let Some(ref store) = self.store {
                    let ids: Vec<String> = session.subagents.clone();
                    let parent = session.parent_agent_id.clone();
                    let created = session.created_at;
                    let sid = session_id.0.clone();
                    drop(sessions);
                    if let Err(e) = store.save_acp_session(&sid, &parent, &ids, created).await {
                        warn!("Failed to persist updated ACP session {}: {}", sid, e);
                    }
                }
            }
        }

        {
            let mut threads = self.threads.write().await;
            if !threads.contains_key(&thread_id) {
                threads.insert(
                    thread_id.clone(),
                    ThreadContext {
                        id: thread_id.clone(),
                        active_subagent: None,
                        created_at: chrono::Utc::now(),
                    },
                );
            }
        }

        drop(subagents);

        // Persist subagent run record if store is attached.
        if let Some(ref store) = self.store {
            if let Err(e) = store
                .save_subagent_run(&SaveSubagentRunParams {
                    run_id: &subagent_id,
                    subagent_id: &subagent_id,
                    session_id: &session_id.to_string(),
                    parent_id: &parent_id,
                    label: None,
                    task_prompt: config.system_prompt.as_deref(),
                    mode: if config.mode == SpawnMode::Run {
                        "run"
                    } else {
                        "session"
                    },
                    thread_id: Some(&thread_id),
                })
                .await
            {
                warn!("Failed to persist subagent run record for {}: {}", subagent_id, e);
            }
            if let Err(e) = store
                .update_subagent_run_status(&subagent_id, "ready")
                .await
            {
                warn!("Failed to update subagent run status for {}: {}", subagent_id, e);
            }
        }

        info!("Subagent {} spawned successfully", subagent_id);
        self.emit(crate::gateway::GatewayEvent::AcpSpawned {
            session_id: session_id.to_string(),
            subagent_id: subagent_id.clone(),
            parent_id: parent_id.clone(),
            mode: format!("{:?}", config.mode).to_lowercase(),
            thread_id: thread_id.clone(),
        })
        .await;
        Ok(handle)
    }

    /// Recover a crashed subagent by spawning a new one with the same config.
    ///
    /// This method may be called externally or by the automatic recovery
    /// watchdog. The backoff delay is the responsibility of the caller; this
    /// method performs the spawn, sets the crash counter on the new handle,
    /// persists a recovery event if a store is attached, and updates the
    /// session's subagent list to point to the replacement.
    pub async fn recover_crashed_subagent(
        &self,
        session_id: crate::acp::config::AcpSessionId,
        parent_id: String,
        config: SubagentConfig,
        crash_count: u32,
    ) -> Option<SubagentHandle> {
        warn!(
            "Recovering crashed subagent (attempt {}, crash_count: {})",
            crash_count + 1,
            crash_count
        );

        let handle = match self
            .spawn_subagent(session_id.clone(), parent_id.clone(), config.clone())
            .await
        {
            Ok(h) => h,
            Err(e) => {
                warn!("Failed to recover crashed subagent: {}", e);
                return None;
            }
        };

        let new_id = handle.id.clone();

        // Follow documented lock order: subagents -> sessions.
        let old_id = {
            let subagents = self.subagents.read().await;
            // Find the predecessor that was previously registered with the same
            // session and parent. We match the first crashed subagent that is
            // still tracked for this session; it should not be the newly spawned
            // replacement.
            subagents
                .values()
                .find(|h| {
                    h.session_id == session_id
                        && h.parent_id == parent_id
                        && h.status == SubagentStatus::Crashed
                        && h.id != new_id
                })
                .map(|h| h.id.clone())
        };

        // The new handle inherits the crash count supplied by the caller
        // (already incremented by the watchdog before recovery is triggered).
        {
            let mut subagents = self.subagents.write().await;
            if let Some(h) = subagents.get_mut(&new_id) {
                h.crash_count = crash_count;
            }
        }

        if let Some(ref old_id) = old_id {
            {
                let mut subagents = self.subagents.write().await;
                subagents.remove(old_id);
            }
            {
                let mut sessions = self.sessions.write().await;
                if let Some(session) = sessions.get_mut(&session_id) {
                    session.subagents.retain(|id| id != old_id);
                    if !session.subagents.contains(&new_id) {
                        session.subagents.push(new_id.clone());
                    }
                    if let Some(ref store) = self.store {
                        let ids = session.subagents.clone();
                        let parent = session.parent_agent_id.clone();
                        let created = session.created_at;
                        let sid = session_id.0.clone();
                        drop(sessions);
                        if let Err(e) = store.save_acp_session(&sid, &parent, &ids, created).await {
                            warn!("Failed to persist recovered ACP session {}: {}", sid, e);
                        }
                    }
                }
            }
        } else {
            // No crashed predecessor found. This can happen if recovery is
            // triggered after the crashed handle was already cleaned up, or if
            // multiple crashed subagents share the session. Guard against
            // adding a duplicate entry in the session list.
            let mut sessions = self.sessions.write().await;
            if let Some(session) = sessions.get_mut(&session_id) {
                if !session.subagents.contains(&new_id) {
                    session.subagents.push(new_id.clone());
                }
            }
        }

        if let Some(ref store) = self.store {
            if let Err(e) = store
                .save_subagent_run(&SaveSubagentRunParams {
                    run_id: &new_id,
                    subagent_id: &new_id,
                    session_id: &session_id.to_string(),
                    parent_id: &handle.parent_id,
                    label: Some("recovery"),
                    task_prompt: Some(&format!(
                        "auto-recovery after crash (attempt {})",
                        crash_count + 1
                    )),
                    mode: if handle.mode == SpawnMode::Run {
                        "run"
                    } else {
                        "session"
                    },
                    thread_id: Some(&handle.thread_id),
                })
                .await
            {
                warn!("Failed to persist recovery run record for {}: {}", new_id, e);
            }
            if let Err(e) = store.update_subagent_run_status(&new_id, "recovered").await {
                warn!("Failed to update recovery run status for {}: {}", new_id, e);
            }
        }

        info!(
            "Crashed subagent recovered successfully (new id: {}, crash_count: {})",
            new_id,
            crash_count + 1
        );
        if let Some(old_id) = old_id {
            self.emit(crate::gateway::GatewayEvent::AcpRecovered {
                session_id: session_id.to_string(),
                old_subagent_id: old_id,
                new_subagent_id: new_id,
                crash_count,
            })
            .await;
        }
        Some(handle)
    }
}

/// Crash-recovery plan for a watchdog: where and how to respawn a crashed
/// subagent through the control-plane actor loop.
struct RecoveryPlan {
    /// Control plane used to schedule the recovery command.
    acp: AcpControlPlane,
    /// Session id to respawn under.
    session_id: crate::acp::config::AcpSessionId,
    /// Parent session of the crashed subagent.
    parent_id: String,
    /// Original subagent configuration to respawn with.
    config: SubagentConfig,
}

/// Watchdog task that monitors a subagent's JoinHandle, updates status on
/// normal exit, panic, or external abort, and triggers automatic crash
/// recovery when configured.
async fn watchdog_task(
    join_handle: tokio::task::JoinHandle<()>,
    subagents: Arc<RwLock<HashMap<String, crate::acp::subagent::SubagentHandle>>>,
    watch_id: String,
    store: Option<Arc<crate::agent::session_store::SessionStore>>,
    acp: AcpControlPlane,
    session_id: crate::acp::config::AcpSessionId,
    recovery: RecoveryPlan,
) {
    type SubagentMap = HashMap<String, SubagentHandle>;
    let RecoveryPlan {
        acp: acp_for_recovery,
        session_id: recovery_session_id,
        parent_id: recovery_parent_id,
        config: recovery_config,
    } = recovery;

    match join_handle.await {
        Ok(()) => {
            let mut map: tokio::sync::RwLockWriteGuard<'_, SubagentMap> = subagents.write().await;
            if let Some(h) = map.get_mut(&watch_id) {
                h.status = SubagentStatus::Terminated;
            }
            drop(map);
            acp.emit(crate::gateway::GatewayEvent::AcpCompleted {
                session_id: session_id.to_string(),
                subagent_id: watch_id.clone(),
                status: "terminated".to_string(),
            })
            .await;
            if let Some(store) = store {
                if let Err(e) = store
                    .complete_subagent_run(&watch_id, Some("normal exit"), None)
                    .await
                {
                    warn!("Failed to persist normal completion for {}: {}", watch_id, e);
                }
            }
        }
        Err(e) if e.is_panic() => {
            warn!("Subagent {} panicked — marking Crashed", watch_id);
            let current_crash_count = {
                let map: tokio::sync::RwLockReadGuard<'_, SubagentMap> = subagents.read().await;
                map.get(&watch_id).map(|h| h.crash_count).unwrap_or(0)
            };
            {
                let mut map: tokio::sync::RwLockWriteGuard<'_, SubagentMap> =
                    subagents.write().await;
                if let Some(h) = map.get_mut(&watch_id) {
                    h.status = SubagentStatus::Crashed;
                }
            }
            acp.emit(crate::gateway::GatewayEvent::AcpCompleted {
                session_id: session_id.to_string(),
                subagent_id: watch_id.clone(),
                status: "crashed".to_string(),
            })
            .await;
            if let Some(ref store) = store {
                if let Err(e) = store
                    .complete_subagent_run(&watch_id, None, Some("panicked"))
                    .await
                {
                    warn!("Failed to persist crash completion for {}: {}", watch_id, e);
                }
            }

            // Automatic recovery with backoff, scheduled through the actor loop
            // to avoid a direct async recursion cycle with spawn_subagent.
            let (should_recover, delay) = {
                let global = acp_for_recovery.recovery.read().await;
                let should_recover = global.enabled && current_crash_count < global.max_retries;
                let delay = if should_recover {
                    global
                        .backoff_seconds
                        .get(current_crash_count as usize)
                        .copied()
                        .unwrap_or_else(|| global.backoff_seconds.last().copied().unwrap_or(30))
                } else {
                    0
                };
                (should_recover, delay)
            };

            if !should_recover {
                return;
            }

            let acp = acp_for_recovery;
            warn!(
                "Auto-recovering subagent {} (attempt {}/{}) in {}s",
                watch_id,
                current_crash_count + 1,
                { acp.recovery.read().await.max_retries },
                delay
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
            let cmd = AcpCommand::RecoverSubagent {
                session_id: recovery_session_id,
                parent_id: recovery_parent_id,
                config: recovery_config,
                crash_count: current_crash_count + 1,
            };
            if let Err(e) = acp.command_tx.send(cmd).await {
                warn!("Failed to schedule recovery command for {}: {}", watch_id, e);
            }
        }
        Err(_) => {
            // Aborted externally
            let mut map: tokio::sync::RwLockWriteGuard<'_, SubagentMap> = subagents.write().await;
            if let Some(h) = map.get_mut(&watch_id) {
                h.status = SubagentStatus::Terminated;
            }
            drop(map);
            acp.emit(crate::gateway::GatewayEvent::AcpCompleted {
                session_id: session_id.to_string(),
                subagent_id: watch_id.clone(),
                status: "aborted".to_string(),
            })
            .await;
        }
    }
}
