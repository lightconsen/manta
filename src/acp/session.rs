use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, info, warn};

use super::config::{AcpSessionId, AcpSessionStatus, ExecutionMode};
use super::control_plane::AcpControlPlane;
use super::controller::ExecutionController;
use crate::agent::{Agent, ProgressCallback};
use crate::channels::{IncomingMessage, OutgoingMessage};

/// Commands sent to the ACP actor
pub enum AcpCommand {
    /// Execute in persistent session mode
    ExecuteSession {
        agent: Arc<Agent>,
        message: IncomingMessage,
        /// Optional per-request max iterations override.
        max_iterations: Option<usize>,
        respond_to: oneshot::Sender<crate::Result<OutgoingMessage>>,
    },
    /// Execute in one-shot run mode
    ExecuteRun {
        agent: Arc<Agent>,
        message: IncomingMessage,
        /// Optional per-request max iterations override.
        max_iterations: Option<usize>,
        respond_to: oneshot::Sender<crate::Result<OutgoingMessage>>,
    },
    /// Execute with progress callbacks in session mode
    ExecuteSessionWithProgress {
        agent: Arc<Agent>,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
        /// Optional per-request max iterations override.
        max_iterations: Option<usize>,
        respond_to: oneshot::Sender<crate::Result<OutgoingMessage>>,
    },
    /// Pause a running session
    Pause { session_id: String },
    /// Resume a paused session
    Resume { session_id: String },
    /// Single step a paused session
    Step { session_id: String },
    /// Cancel a running session
    Cancel { session_id: String },
    /// Get session status
    GetStatus {
        session_id: String,
        respond_to: oneshot::Sender<Option<AcpSessionStatus>>,
    },
    /// Execute a message on behalf of the channel bridge.
    ///
    /// The ACP actor resolves an agent via the configured default agent
    /// builder, routes to the requested session, and returns the resulting
    /// outgoing message on the provided channel.
    ExecuteForBridge {
        session_id: String,
        message: IncomingMessage,
        mode: ExecutionMode,
        respond_to: oneshot::Sender<crate::Result<OutgoingMessage>>,
    },
    /// Recover a crashed subagent.
    RecoverSubagent {
        session_id: AcpSessionId,
        parent_id: String,
        config: super::config::SubagentConfig,
        crash_count: u32,
    },
    /// Shutdown the ACP
    Shutdown,
}

impl std::fmt::Debug for AcpCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcpCommand::ExecuteSession { message, respond_to: _, .. } => f
                .debug_struct("ExecuteSession")
                .field("message", message)
                .finish(),
            AcpCommand::ExecuteRun { message, respond_to: _, .. } => f
                .debug_struct("ExecuteRun")
                .field("message", message)
                .finish(),
            AcpCommand::ExecuteSessionWithProgress { message, respond_to: _, .. } => f
                .debug_struct("ExecuteSessionWithProgress")
                .field("message", message)
                .finish(),
            AcpCommand::Pause { session_id } => f
                .debug_struct("Pause")
                .field("session_id", session_id)
                .finish(),
            AcpCommand::Resume { session_id } => f
                .debug_struct("Resume")
                .field("session_id", session_id)
                .finish(),
            AcpCommand::Step { session_id } => f
                .debug_struct("Step")
                .field("session_id", session_id)
                .finish(),
            AcpCommand::Cancel { session_id } => f
                .debug_struct("Cancel")
                .field("session_id", session_id)
                .finish(),
            AcpCommand::GetStatus { session_id, .. } => f
                .debug_struct("GetStatus")
                .field("session_id", session_id)
                .finish(),
            AcpCommand::ExecuteForBridge { session_id, message, mode, .. } => f
                .debug_struct("ExecuteForBridge")
                .field("session_id", session_id)
                .field("message", message)
                .field("mode", mode)
                .finish(),
            AcpCommand::RecoverSubagent {
                session_id,
                parent_id,
                crash_count,
                ..
            } => f
                .debug_struct("RecoverSubagent")
                .field("session_id", session_id)
                .field("parent_id", parent_id)
                .field("crash_count", crash_count)
                .finish(),
            AcpCommand::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// Payload for the `SessionCommand::Execute` variant.
/// Boxed to keep the enum size small.
pub(crate) struct SessionExecutePayload {
    pub(crate) agent: Arc<Agent>,
    pub(crate) message: IncomingMessage,
    pub(crate) mode: ExecutionMode,
    pub(crate) progress_cb: Option<ProgressCallback>,
    /// Effective max iteration limit for this specific execution.
    pub(crate) max_iterations: usize,
    pub(crate) respond_to: oneshot::Sender<crate::Result<OutgoingMessage>>,
}

/// Internal command sent to a per-session actor
pub(crate) enum SessionCommand {
    Execute(Box<SessionExecutePayload>),
    Shutdown,
}

impl std::fmt::Debug for SessionCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionCommand::Execute(payload) => f
                .debug_struct("Execute")
                .field("message", &payload.message)
                .field("mode", &payload.mode)
                .finish(),
            SessionCommand::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// Handle to a running session actor
#[derive(Debug)]
pub(crate) struct SessionHandle {
    pub(crate) tx: mpsc::Sender<SessionCommand>,
    pub(crate) controller: Arc<ExecutionController>,
    pub(crate) mode: ExecutionMode,
}

/// Context passed to the ACP actor loop.
///
/// This is a narrow facade over the parts of `AcpControlPlane` that the
/// session actor actually needs. It breaks the circular dependency between
/// `session` and `control_plane` modules.
#[derive(Clone)]
pub(crate) struct ActorContext {
    pub(crate) max_iterations: usize,
    #[allow(clippy::type_complexity)]
    pub(crate) default_agent_builder:
        Arc<RwLock<Option<Arc<dyn Fn() -> crate::Result<Agent> + Send + Sync>>>>,
    pub(crate) control_plane: AcpControlPlane,
}

/// ACP actor loop — routes commands to per-session serial queues
pub(crate) async fn acp_actor_loop(mut command_rx: mpsc::Receiver<AcpCommand>, ctx: ActorContext) {
    let mut sessions: HashMap<String, SessionHandle> = HashMap::new();
    let mut session_meta: HashMap<String, usize> = HashMap::new();

    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            AcpCommand::ExecuteSession {
                agent,
                message,
                max_iterations: req_max_iter,
                respond_to,
            } => {
                let session_id = message.conversation_id.0.clone();
                let effective_max = req_max_iter.unwrap_or(ctx.max_iterations);
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Session,
                    effective_max,
                )
                .await;

                if let Err(e) = handle
                    .tx
                    .send(SessionCommand::Execute(Box::new(SessionExecutePayload {
                        agent,
                        message,
                        mode: ExecutionMode::Session,
                        progress_cb: None,
                        max_iterations: effective_max,
                        respond_to,
                    })))
                    .await
                {
                    warn!("Failed to dispatch ExecuteSession for {}: {}", session_id, e);
                }
            }

            AcpCommand::ExecuteRun {
                agent,
                message,
                max_iterations: req_max_iter,
                respond_to,
            } => {
                let session_id = message.conversation_id.0.clone();
                let effective_max = req_max_iter.unwrap_or(ctx.max_iterations);
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Run,
                    effective_max,
                )
                .await;

                if let Err(e) = handle
                    .tx
                    .send(SessionCommand::Execute(Box::new(SessionExecutePayload {
                        agent,
                        message,
                        mode: ExecutionMode::Run,
                        progress_cb: None,
                        max_iterations: effective_max,
                        respond_to,
                    })))
                    .await
                {
                    warn!("Failed to dispatch ExecuteRun for {}: {}", session_id, e);
                }
            }

            AcpCommand::ExecuteSessionWithProgress {
                agent,
                message,
                progress_cb,
                max_iterations: req_max_iter,
                respond_to,
            } => {
                let session_id = message.conversation_id.0.clone();
                let effective_max = req_max_iter.unwrap_or(ctx.max_iterations);
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Session,
                    effective_max,
                )
                .await;

                if let Err(e) = handle
                    .tx
                    .send(SessionCommand::Execute(Box::new(SessionExecutePayload {
                        agent,
                        message,
                        mode: ExecutionMode::Session,
                        progress_cb: Some(progress_cb),
                        max_iterations: effective_max,
                        respond_to,
                    })))
                    .await
                {
                    warn!(
                        "Failed to dispatch ExecuteSessionWithProgress for {}: {}",
                        session_id, e
                    );
                }
            }

            AcpCommand::Pause { session_id } => {
                if let Some(handle) = sessions.get(&session_id) {
                    handle.controller.pause().await;
                    let state = handle.controller.current_state().await;
                    ctx.control_plane
                        .emit(crate::gateway::GatewayEvent::AcpStatusChanged {
                            session_id,
                            runtime_state: state.to_string(),
                        })
                        .await;
                }
            }

            AcpCommand::Resume { session_id } => {
                if let Some(handle) = sessions.get(&session_id) {
                    handle.controller.resume().await;
                    let state = handle.controller.current_state().await;
                    ctx.control_plane
                        .emit(crate::gateway::GatewayEvent::AcpStatusChanged {
                            session_id,
                            runtime_state: state.to_string(),
                        })
                        .await;
                }
            }

            AcpCommand::Step { session_id } => {
                if let Some(handle) = sessions.get(&session_id) {
                    handle.controller.step().await;
                    let state = handle.controller.current_state().await;
                    ctx.control_plane
                        .emit(crate::gateway::GatewayEvent::AcpStatusChanged {
                            session_id,
                            runtime_state: state.to_string(),
                        })
                        .await;
                }
            }

            AcpCommand::Cancel { session_id } => {
                if let Some(handle) = sessions.get(&session_id) {
                    handle.controller.cancel().await;
                    let state = handle.controller.current_state().await;
                    ctx.control_plane
                        .emit(crate::gateway::GatewayEvent::AcpStatusChanged {
                            session_id,
                            runtime_state: state.to_string(),
                        })
                        .await;
                }
            }

            AcpCommand::GetStatus { session_id, respond_to } => {
                let status = if let Some(handle) = sessions.get(&session_id) {
                    let queue_depth = 256_usize.saturating_sub(handle.tx.capacity());
                    let max_iterations = session_meta
                        .get(&session_id)
                        .copied()
                        .unwrap_or(ctx.max_iterations);
                    Some(AcpSessionStatus {
                        session_id: session_id.clone(),
                        runtime_state: handle.controller.current_state().await,
                        mode: handle.mode,
                        current_iteration: handle.controller.current_iteration(),
                        max_iterations,
                        queue_depth,
                    })
                } else {
                    None
                };
                if let Err(e) = respond_to.send(status) {
                    warn!("Failed to send GetStatus response for {}: {:?}", session_id, e);
                }
            }

            AcpCommand::ExecuteForBridge {
                session_id,
                message,
                mode,
                respond_to,
            } => {
                let agent = {
                    let builder_guard = ctx.default_agent_builder.read().await;
                    match builder_guard.as_ref() {
                        Some(builder) => match builder() {
                            Ok(agent) => Arc::new(agent),
                            Err(e) => {
                                if let Err(send_err) = respond_to.send(Err(e)) {
                                    warn!(
                                        "Failed to send ExecuteForBridge builder error response: \
                                         {:?}",
                                        send_err
                                    );
                                }
                                continue;
                            }
                        },
                        None => {
                            let err = crate::error::SyscityError::Internal(
                                "No agent builder configured".to_string(),
                            );
                            if let Err(send_err) = respond_to.send(Err(err)) {
                                warn!(
                                    "Failed to send ExecuteForBridge no-builder error response: \
                                     {:?}",
                                    send_err
                                );
                            }
                            continue;
                        }
                    }
                };

                let effective_max = ctx.max_iterations;
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    mode,
                    effective_max,
                )
                .await;

                if let Err(e) = handle
                    .tx
                    .send(SessionCommand::Execute(Box::new(SessionExecutePayload {
                        agent,
                        message,
                        mode,
                        progress_cb: None,
                        max_iterations: effective_max,
                        respond_to,
                    })))
                    .await
                {
                    warn!("Failed to dispatch ExecuteForBridge for {}: {}", session_id, e);
                }
            }

            AcpCommand::RecoverSubagent {
                session_id,
                parent_id,
                config,
                crash_count,
            } => {
                let result = ctx
                    .control_plane
                    .recover_crashed_subagent(session_id, parent_id, config, crash_count)
                    .await;
                if result.is_none() {
                    warn!("Failed to recover crashed subagent");
                }
            }

            AcpCommand::Shutdown => {
                info!("ACP actor shutting down");
                for handle in sessions.values() {
                    handle.controller.cancel().await;
                    if let Err(e) = handle.tx.send(SessionCommand::Shutdown).await {
                        warn!("Failed to send Shutdown to session actor: {}", e);
                    }
                }
                sessions.clear();
                session_meta.clear();

                // Also shut down all subagents managed by the control plane.
                let subagents = ctx.control_plane.subagents.read().await;
                for handle in subagents.values() {
                    if let Err(e) = handle
                        .command_tx
                        .send(super::subagent::SubagentCommand::Shutdown)
                        .await
                    {
                        warn!("Failed to send Shutdown to subagent {}: {}", handle.id, e);
                    }
                }
                drop(subagents);

                break;
            }
        }
    }
}

/// Get or create a session actor for the given session_id.
pub(crate) async fn get_or_create_session<'a>(
    sessions: &'a mut HashMap<String, SessionHandle>,
    session_meta: &'a mut HashMap<String, usize>,
    session_id: &str,
    mode: ExecutionMode,
    max_iterations: usize,
) -> &'a SessionHandle {
    if !sessions.contains_key(session_id) {
        let (tx, rx) = mpsc::channel::<SessionCommand>(256);
        let controller = ExecutionController::new();
        let ctrl_clone = controller.clone();

        let meta = max_iterations;

        let handle = SessionHandle {
            tx,
            controller: controller.clone(),
            mode,
        };

        tokio::spawn(session_actor_loop(rx, ctrl_clone, session_id.to_string()));

        sessions.insert(session_id.to_string(), handle);
        session_meta.insert(session_id.to_string(), meta);
    }

    #[allow(clippy::expect_used)] // just inserted above when missing
    sessions
        .get(session_id)
        .expect("session handle inserted above")
}

/// Per-session actor loop — processes messages serially for one session.
pub(crate) async fn session_actor_loop(
    mut rx: mpsc::Receiver<SessionCommand>,
    controller: Arc<ExecutionController>,
    session_id: String,
) {
    info!("Session actor started for {}", session_id);

    while let Some(cmd) = rx.recv().await {
        match cmd {
            SessionCommand::Execute(payload) => {
                let msg_preview = payload.message.content.chars().take(60).collect::<String>();
                debug!(
                    "Session {} executing {} mode message: {}...",
                    session_id,
                    if payload.mode == ExecutionMode::Session {
                        "session"
                    } else {
                        "run"
                    },
                    msg_preview
                );

                controller.reset().await;
                let max_iter = payload.max_iterations;

                let result = if let Some(cb) = payload.progress_cb {
                    payload
                        .agent
                        .process_message_with_progress_and_controller(
                            payload.message,
                            cb,
                            controller.clone(),
                            max_iter,
                        )
                        .await
                } else if payload.mode == ExecutionMode::Run {
                    payload
                        .agent
                        .run_message_with_controller(payload.message, controller.clone(), max_iter)
                        .await
                } else {
                    payload
                        .agent
                        .process_message_with_controller(
                            payload.message,
                            controller.clone(),
                            max_iter,
                        )
                        .await
                };

                controller.reset().await;

                if let Err(ref e) = result {
                    warn!("Session {} execution error: {}", session_id, e);
                }

                if let Err(e) = payload.respond_to.send(result) {
                    warn!("Session {} failed to send execution result: {:?}", session_id, e);
                }
            }

            SessionCommand::Shutdown => {
                info!("Session actor shutting down for {}", session_id);
                break;
            }
        }
    }

    info!("Session actor ended for {}", session_id);
}
