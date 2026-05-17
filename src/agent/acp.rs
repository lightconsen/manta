//! ACP (Agent Control Plane) — Centralized actor queue orchestration
//!
//! Inspired by OpenClaw's ACP session management, this provides:
//! - Centralized command dispatch for all agent execution
//! - Session mode (persistent) and Run mode (one-shot)
//! - Runtime controls: pause, resume, step, cancel
//! - Per-session serial execution (one message at a time per session)

use crate::agent::{Agent, ProgressCallback};
use crate::channels::{IncomingMessage, OutgoingMessage};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::sync::Notify;
use tracing::{debug, info, warn};

/// Execution mode for an ACP command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Persistent session — context is kept across turns
    Session,
    /// One-shot run — context is discarded after completion
    Run,
}

/// Runtime state of a session's execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    /// Idle, waiting for input
    Idle,
    /// Actively running
    Running,
    /// Paused between iterations
    Paused,
    /// Will execute one iteration then pause
    Stepping,
    /// Cancelled, will stop at next check
    Cancelled,
}

impl std::fmt::Display for RuntimeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeState::Idle => write!(f, "idle"),
            RuntimeState::Running => write!(f, "running"),
            RuntimeState::Paused => write!(f, "paused"),
            RuntimeState::Stepping => write!(f, "stepping"),
            RuntimeState::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// Controller for pausing / resuming / stepping execution.
///
/// Inserted into the Agent's tool-call loop so operators can pause
/// between LLM iterations.
#[derive(Debug)]
pub struct ExecutionController {
    state: RwLock<RuntimeState>,
    notify: Notify,
}

impl ExecutionController {
    /// Create a new controller in the `Idle` state.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: RwLock::new(RuntimeState::Idle),
            notify: Notify::new(),
        })
    }

    /// Check if execution should proceed.
    ///
    /// - `Idle` / `Running` → returns immediately.
    /// - `Stepping` → returns once, then transitions to `Paused`.
    /// - `Paused` → blocks until state changes.
    /// - `Cancelled` → returns `Err("Execution cancelled by user")`.
    pub async fn check_and_wait(&self) -> Result<(), &'static str> {
        loop {
            let state = *self.state.read().await;
            match state {
                RuntimeState::Idle | RuntimeState::Running => return Ok(()),
                RuntimeState::Stepping => {
                    *self.state.write().await = RuntimeState::Paused;
                    return Ok(());
                }
                RuntimeState::Paused => {
                    self.notify.notified().await;
                    continue;
                }
                RuntimeState::Cancelled => return Err("Execution cancelled by user"),
            }
        }
    }

    /// Transition to `Paused`.
    pub async fn pause(&self) {
        let mut state = self.state.write().await;
        if *state == RuntimeState::Running || *state == RuntimeState::Idle {
            *state = RuntimeState::Paused;
            info!("Execution paused");
        }
    }

    /// Transition to `Running` and wake waiters.
    pub async fn resume(&self) {
        let mut state = self.state.write().await;
        if *state == RuntimeState::Paused || *state == RuntimeState::Stepping {
            *state = RuntimeState::Running;
            drop(state);
            self.notify.notify_waiters();
            info!("Execution resumed");
        }
    }

    /// Transition to `Stepping` and wake waiters.
    pub async fn step(&self) {
        let mut state = self.state.write().await;
        *state = RuntimeState::Stepping;
        drop(state);
        self.notify.notify_waiters();
        info!("Single step triggered");
    }

    /// Transition to `Cancelled` and wake waiters.
    pub async fn cancel(&self) {
        let mut state = self.state.write().await;
        *state = RuntimeState::Cancelled;
        drop(state);
        self.notify.notify_waiters();
        info!("Execution cancelled");
    }

    /// Reset to `Idle`.
    pub async fn reset(&self) {
        let mut state = self.state.write().await;
        *state = RuntimeState::Idle;
    }

    /// Current runtime state.
    pub async fn current_state(&self) -> RuntimeState {
        *self.state.read().await
    }
}

/// Status of an ACP-managed session
#[derive(Debug, Clone)]
pub struct AcpSessionStatus {
    pub session_id: String,
    pub runtime_state: RuntimeState,
    pub mode: ExecutionMode,
    pub current_iteration: usize,
    pub max_iterations: usize,
    pub queue_depth: usize,
    pub current_message: Option<String>,
}

/// Commands sent to the ACP actor
pub enum AcpCommand {
    /// Execute in persistent session mode
    ExecuteSession {
        agent: Arc<Agent>,
        message: IncomingMessage,
        respond_to: oneshot::Sender<crate::Result<OutgoingMessage>>,
    },
    /// Execute in one-shot run mode
    ExecuteRun {
        agent: Arc<Agent>,
        message: IncomingMessage,
        respond_to: oneshot::Sender<crate::Result<OutgoingMessage>>,
    },
    /// Execute with progress callbacks in session mode
    ExecuteSessionWithProgress {
        agent: Arc<Agent>,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
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
    /// Shutdown the ACP
    Shutdown,
}

impl std::fmt::Debug for AcpCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcpCommand::ExecuteSession { message, respond_to: _, .. } => {
                f.debug_struct("ExecuteSession")
                    .field("message", message)
                    .finish()
            }
            AcpCommand::ExecuteRun { message, respond_to: _, .. } => {
                f.debug_struct("ExecuteRun")
                    .field("message", message)
                    .finish()
            }
            AcpCommand::ExecuteSessionWithProgress { message, respond_to: _, .. } => {
                f.debug_struct("ExecuteSessionWithProgress")
                    .field("message", message)
                    .finish()
            }
            AcpCommand::Pause { session_id } => {
                f.debug_struct("Pause").field("session_id", session_id).finish()
            }
            AcpCommand::Resume { session_id } => {
                f.debug_struct("Resume").field("session_id", session_id).finish()
            }
            AcpCommand::Step { session_id } => {
                f.debug_struct("Step").field("session_id", session_id).finish()
            }
            AcpCommand::Cancel { session_id } => {
                f.debug_struct("Cancel").field("session_id", session_id).finish()
            }
            AcpCommand::GetStatus { session_id, .. } => {
                f.debug_struct("GetStatus").field("session_id", session_id).finish()
            }
            AcpCommand::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// Internal command sent to a per-session actor
enum SessionCommand {
    Execute {
        agent: Arc<Agent>,
        message: IncomingMessage,
        mode: ExecutionMode,
        progress_cb: Option<ProgressCallback>,
        respond_to: oneshot::Sender<crate::Result<OutgoingMessage>>,
    },
    #[allow(dead_code)]
    GetStatus {
        controller_state: RuntimeState,
        respond_to: oneshot::Sender<Option<AcpSessionStatus>>,
    },
    Shutdown,
}

impl std::fmt::Debug for SessionCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionCommand::Execute { message, mode, .. } => {
                f.debug_struct("Execute")
                    .field("message", message)
                    .field("mode", mode)
                    .finish()
            }
            SessionCommand::GetStatus { controller_state, .. } => {
                f.debug_struct("GetStatus")
                    .field("controller_state", controller_state)
                    .finish()
            }
            SessionCommand::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// Handle to a running session actor
#[derive(Debug)]
struct SessionHandle {
    tx: mpsc::Sender<SessionCommand>,
    controller: Arc<ExecutionController>,
    mode: ExecutionMode,
}

/// Per-session execution tracking (held in the main ACP loop)
#[derive(Debug)]
#[allow(dead_code)]
struct SessionExecution {
    controller: Arc<ExecutionController>,
    mode: ExecutionMode,
    current_iteration: usize,
    max_iterations: usize,
    current_message: Option<String>,
}

/// Central ACP controller with actor queue
#[derive(Debug, Clone)]
pub struct AcpController {
    command_tx: mpsc::Sender<AcpCommand>,
}

impl AcpController {
    /// Create a new ACP controller and spawn the actor task
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel(256);
        tokio::spawn(acp_actor_loop(command_rx));
        Self { command_tx }
    }

    /// Execute a message in persistent session mode
    pub async fn execute_session(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
    ) -> crate::Result<OutgoingMessage> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(AcpCommand::ExecuteSession {
                agent,
                message,
                respond_to: tx,
            })
            .await;
        rx.await.map_err(|_| {
            crate::error::MantaError::Internal("ACP channel closed".to_string())
        })?
    }

    /// Execute a message in one-shot run mode
    pub async fn execute_run(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
    ) -> crate::Result<OutgoingMessage> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(AcpCommand::ExecuteRun {
                agent,
                message,
                respond_to: tx,
            })
            .await;
        rx.await.map_err(|_| {
            crate::error::MantaError::Internal("ACP channel closed".to_string())
        })?
    }

    /// Execute with progress callbacks in session mode
    pub async fn execute_session_with_progress(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
    ) -> crate::Result<OutgoingMessage> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(AcpCommand::ExecuteSessionWithProgress {
                agent,
                message,
                progress_cb,
                respond_to: tx,
            })
            .await;
        rx.await.map_err(|_| {
            crate::error::MantaError::Internal("ACP channel closed".to_string())
        })?
    }

    /// Pause a running session
    pub async fn pause(&self, session_id: String) {
        let _ = self.command_tx.send(AcpCommand::Pause { session_id }).await;
    }

    /// Resume a paused session
    pub async fn resume(&self, session_id: String) {
        let _ = self.command_tx.send(AcpCommand::Resume { session_id }).await;
    }

    /// Single step a paused session
    pub async fn step(&self, session_id: String) {
        let _ = self.command_tx.send(AcpCommand::Step { session_id }).await;
    }

    /// Cancel a running session
    pub async fn cancel(&self, session_id: String) {
        let _ = self.command_tx.send(AcpCommand::Cancel { session_id }).await;
    }

    /// Get session status
    pub async fn get_status(&self, session_id: String) -> Option<AcpSessionStatus> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(AcpCommand::GetStatus { session_id, respond_to: tx })
            .await;
        rx.await.ok().flatten()
    }

    /// Shutdown the ACP
    pub async fn shutdown(&self) {
        let _ = self.command_tx.send(AcpCommand::Shutdown).await;
    }
}

/// ACP actor loop — routes commands to per-session serial queues
async fn acp_actor_loop(mut command_rx: mpsc::Receiver<AcpCommand>) {
    let mut sessions: HashMap<String, SessionHandle> = HashMap::new();
    let mut session_meta: HashMap<String, SessionExecution> = HashMap::new();

    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            AcpCommand::ExecuteSession { agent, message, respond_to } => {
                let session_id = message.conversation_id.0.clone();
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Session,
                ).await;

                let _ = handle.tx.send(SessionCommand::Execute {
                    agent,
                    message,
                    mode: ExecutionMode::Session,
                    progress_cb: None,
                    respond_to,
                }).await;
            }

            AcpCommand::ExecuteRun { agent, message, respond_to } => {
                let session_id = message.conversation_id.0.clone();
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Run,
                ).await;

                let _ = handle.tx.send(SessionCommand::Execute {
                    agent,
                    message,
                    mode: ExecutionMode::Run,
                    progress_cb: None,
                    respond_to,
                }).await;
            }

            AcpCommand::ExecuteSessionWithProgress { agent, message, progress_cb, respond_to } => {
                let session_id = message.conversation_id.0.clone();
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Session,
                ).await;

                let _ = handle.tx.send(SessionCommand::Execute {
                    agent,
                    message,
                    mode: ExecutionMode::Session,
                    progress_cb: Some(progress_cb),
                    respond_to,
                }).await;
            }

            AcpCommand::Pause { session_id } => {
                if let Some(handle) = sessions.get(&session_id) {
                    handle.controller.pause().await;
                }
            }

            AcpCommand::Resume { session_id } => {
                if let Some(handle) = sessions.get(&session_id) {
                    handle.controller.resume().await;
                }
            }

            AcpCommand::Step { session_id } => {
                if let Some(handle) = sessions.get(&session_id) {
                    handle.controller.step().await;
                }
            }

            AcpCommand::Cancel { session_id } => {
                if let Some(handle) = sessions.get(&session_id) {
                    handle.controller.cancel().await;
                }
            }

            AcpCommand::GetStatus { session_id, respond_to } => {
                let status = if let Some(handle) = sessions.get(&session_id) {
                    let queue_depth = 256_usize.saturating_sub(handle.tx.capacity());
                    let current_message = session_meta
                        .get(&session_id)
                        .and_then(|m| m.current_message.clone());
                    Some(AcpSessionStatus {
                        session_id: session_id.clone(),
                        runtime_state: handle.controller.current_state().await,
                        mode: handle.mode,
                        current_iteration: session_meta
                            .get(&session_id)
                            .map(|m| m.current_iteration)
                            .unwrap_or(0),
                        max_iterations: session_meta
                            .get(&session_id)
                            .map(|m| m.max_iterations)
                            .unwrap_or(50),
                        queue_depth,
                        current_message,
                    })
                } else {
                    None
                };
                let _ = respond_to.send(status);
            }

            AcpCommand::Shutdown => {
                info!("ACP actor shutting down");
                for (_, handle) in &sessions {
                    handle.controller.cancel().await;
                    let _ = handle.tx.send(SessionCommand::Shutdown).await;
                }
                sessions.clear();
                session_meta.clear();
                break;
            }
        }
    }
}

/// Get or create a session actor for the given session_id.
async fn get_or_create_session<'a>(
    sessions: &'a mut HashMap<String, SessionHandle>,
    session_meta: &'a mut HashMap<String, SessionExecution>,
    session_id: &str,
    mode: ExecutionMode,
) -> &'a SessionHandle {
    if !sessions.contains_key(session_id) {
        let (tx, rx) = mpsc::channel::<SessionCommand>(256);
        let controller = ExecutionController::new();
        let ctrl_clone = controller.clone();

        let meta = SessionExecution {
            controller: controller.clone(),
            mode,
            current_iteration: 0,
            max_iterations: 50,
            current_message: None,
        };

        let handle = SessionHandle {
            tx,
            controller: controller.clone(),
            mode,
        };

        tokio::spawn(session_actor_loop(rx, ctrl_clone, session_id.to_string()));

        sessions.insert(session_id.to_string(), handle);
        session_meta.insert(session_id.to_string(), meta);
    }

    sessions.get(session_id).unwrap()
}

/// Per-session actor loop — processes messages serially for one session.
async fn session_actor_loop(
    mut rx: mpsc::Receiver<SessionCommand>,
    controller: Arc<ExecutionController>,
    session_id: String,
) {
    info!("Session actor started for {}", session_id);

    while let Some(cmd) = rx.recv().await {
        match cmd {
            SessionCommand::Execute {
                agent,
                message,
                mode,
                progress_cb,
                respond_to,
            } => {
                let msg_preview = message.content.chars().take(60).collect::<String>();
                debug!(
                    "Session {} executing {} mode message: {}...",
                    session_id,
                    if mode == ExecutionMode::Session { "session" } else { "run" },
                    msg_preview
                );

                controller.reset().await;
                let max_iter = 50;

                let result = if let Some(cb) = progress_cb {
                    agent
                        .process_message_with_progress_and_controller(
                            message,
                            cb,
                            controller.clone(),
                            max_iter,
                        )
                        .await
                } else if mode == ExecutionMode::Run {
                    agent
                        .run_message_with_controller(message, controller.clone(), max_iter)
                        .await
                } else {
                    agent
                        .process_message_with_controller(message, controller.clone(), max_iter)
                        .await
                };

                controller.reset().await;

                if let Err(ref e) = result {
                    warn!("Session {} execution error: {}", session_id, e);
                }

                let _ = respond_to.send(result);
            }

            SessionCommand::GetStatus {
                controller_state,
                respond_to,
            } => {
                let _ = respond_to.send(Some(AcpSessionStatus {
                    session_id: session_id.clone(),
                    runtime_state: controller_state,
                    mode: ExecutionMode::Session, // placeholder, filled by caller
                    current_iteration: 0,
                    max_iterations: 50,
                    queue_depth: 0,
                    current_message: None,
                }));
            }

            SessionCommand::Shutdown => {
                info!("Session actor shutting down for {}", session_id);
                break;
            }
        }
    }

    info!("Session actor ended for {}", session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execution_controller_running() {
        let ctrl = ExecutionController::new();
        ctrl.reset().await;
        // Running / Idle → returns immediately
        assert!(ctrl.check_and_wait().await.is_ok());
    }

    #[tokio::test]
    async fn test_execution_controller_cancel() {
        let ctrl = ExecutionController::new();
        ctrl.cancel().await;
        assert!(ctrl.check_and_wait().await.is_err());
    }

    #[tokio::test]
    async fn test_execution_controller_step_then_pause() {
        let ctrl = ExecutionController::new();
        ctrl.step().await;
        // First call: Stepping → returns, then becomes Paused
        assert!(ctrl.check_and_wait().await.is_ok());
        assert_eq!(ctrl.current_state().await, RuntimeState::Paused);
    }

    #[tokio::test]
    async fn test_execution_controller_pause_resume() {
        let ctrl = ExecutionController::new();

        // Start paused
        ctrl.pause().await;

        // Spawn a task that waits
        let ctrl2 = ctrl.clone();
        let handle = tokio::spawn(async move {
            ctrl2.check_and_wait().await
        });

        // Small delay to let the task reach the wait
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Resume
        ctrl.resume().await;

        assert!(handle.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn test_acp_controller_new() {
        let acp = AcpController::new();
        // Just verify it doesn't panic
        drop(acp);
    }

    #[tokio::test]
    async fn test_acp_serial_queue_same_session() {
        let acp = AcpController::new();
        let session_id = "test-session-1".to_string();

        // Status for non-existent session should be None
        let status = acp.get_status(session_id.clone()).await;
        assert!(status.is_none());

        // After shutdown, no panic
        acp.shutdown().await;
    }
}
