//! ACP (Agent Control Plane) — Centralized actor queue orchestration
//!
//! Inspired by OpenClaw's ACP session management, this provides:
//! - Centralized command dispatch for all agent execution
//! - Session mode (persistent) and Run mode (one-shot)
//! - Runtime controls: pause, resume, step, cancel
//! - Per-session execution state tracking

use crate::agent::{Agent, ProgressCallback};
use crate::channels::{IncomingMessage, OutgoingMessage};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

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

/// Per-session execution tracking
#[derive(Debug)]
struct SessionExecution {
    controller: Arc<ExecutionController>,
    mode: ExecutionMode,
    current_iteration: usize,
    max_iterations: usize,
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

/// ACP actor loop — single-threaded command processor
async fn acp_actor_loop(mut command_rx: mpsc::Receiver<AcpCommand>) {
    let mut sessions: HashMap<String, SessionExecution> = HashMap::new();

    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            AcpCommand::ExecuteSession { agent, message, respond_to } => {
                let session_id = message.conversation_id.0.clone();

                let controller = if let Some(exec) = sessions.get(&session_id) {
                    exec.controller.clone()
                } else {
                    let ctrl = ExecutionController::new();
                    sessions.insert(session_id.clone(), SessionExecution {
                        controller: ctrl.clone(),
                        mode: ExecutionMode::Session,
                        current_iteration: 0,
                        max_iterations: 50,
                    });
                    ctrl
                };

                controller.reset().await;
                let max_iter = sessions.get(&session_id).unwrap().max_iterations;

                tokio::spawn(async move {
                    let result = agent
                        .process_message_with_controller(message, controller.clone(), max_iter)
                        .await;
                    let _ = respond_to.send(result);
                    controller.reset().await;
                });
            }

            AcpCommand::ExecuteRun { agent, message, respond_to } => {
                let controller = ExecutionController::new();
                let max_iter = 50;

                tokio::spawn(async move {
                    let result = agent
                        .run_message_with_controller(message, controller.clone(), max_iter)
                        .await;
                    let _ = respond_to.send(result);
                });
            }

            AcpCommand::ExecuteSessionWithProgress { agent, message, progress_cb, respond_to } => {
                let session_id = message.conversation_id.0.clone();

                let controller = if let Some(exec) = sessions.get(&session_id) {
                    exec.controller.clone()
                } else {
                    let ctrl = ExecutionController::new();
                    sessions.insert(session_id.clone(), SessionExecution {
                        controller: ctrl.clone(),
                        mode: ExecutionMode::Session,
                        current_iteration: 0,
                        max_iterations: 50,
                    });
                    ctrl
                };

                controller.reset().await;
                let max_iter = sessions.get(&session_id).unwrap().max_iterations;

                tokio::spawn(async move {
                    let result = agent
                        .process_message_with_progress_and_controller(
                            message,
                            progress_cb,
                            controller.clone(),
                            max_iter,
                        )
                        .await;
                    let _ = respond_to.send(result);
                    controller.reset().await;
                });
            }

            AcpCommand::Pause { session_id } => {
                if let Some(exec) = sessions.get(&session_id) {
                    exec.controller.pause().await;
                }
            }

            AcpCommand::Resume { session_id } => {
                if let Some(exec) = sessions.get(&session_id) {
                    exec.controller.resume().await;
                }
            }

            AcpCommand::Step { session_id } => {
                if let Some(exec) = sessions.get(&session_id) {
                    exec.controller.step().await;
                }
            }

            AcpCommand::Cancel { session_id } => {
                if let Some(exec) = sessions.get(&session_id) {
                    exec.controller.cancel().await;
                }
            }

            AcpCommand::GetStatus { session_id, respond_to } => {
                let status = if let Some(exec) = sessions.get(&session_id) {
                    Some(AcpSessionStatus {
                        session_id: session_id.clone(),
                        runtime_state: exec.controller.current_state().await,
                        mode: exec.mode,
                        current_iteration: exec.current_iteration,
                        max_iterations: exec.max_iterations,
                    })
                } else {
                    None
                };
                let _ = respond_to.send(status);
            }

            AcpCommand::Shutdown => {
                info!("ACP actor shutting down");
                for (_, exec) in &sessions {
                    exec.controller.cancel().await;
                }
                break;
            }
        }
    }
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
}
