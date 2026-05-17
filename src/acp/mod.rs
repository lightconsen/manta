//! Agent Control Plane (ACP) - Subagent Spawning System
//!
//! Inspired by OpenClaw's ACP, this provides:
//! - Subagent spawning with thread binding
//! - Runtime modes: "run" (one-shot) vs "session" (persistent)
//! - Session actor queue for serialized execution
//! - Parent-child agent communication

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::agent::{Agent, AgentConfig, ProgressCallback};
use crate::channels::{IncomingMessage, OutgoingMessage};

// AgentHandle is defined in gateway module
pub use crate::gateway::AgentHandle;

/// ACP Session ID - unique identifier for an ACP session
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AcpSessionId(pub String);

impl AcpSessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for AcpSessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AcpSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Subagent spawn mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnMode {
    /// One-shot execution (run and terminate)
    Run,
    /// Persistent session (long-running)
    Session,
}

impl Default for SpawnMode {
    fn default() -> Self {
        SpawnMode::Run
    }
}

/// Thread binding mode for subagents
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadBinding {
    /// New isolated thread
    New,
    /// Bind to parent's thread
    Parent,
    /// Bind to specific thread ID
    Thread(String),
    /// Automatic based on context
    Auto,
}

impl Default for ThreadBinding {
    fn default() -> Self {
        ThreadBinding::Auto
    }
}

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
    notify: tokio::sync::Notify,
}

impl ExecutionController {
    /// Create a new controller in the `Idle` state.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: RwLock::new(RuntimeState::Idle),
            notify: tokio::sync::Notify::new(),
        })
    }

    /// Check if execution should proceed.
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
            AcpCommand::Shutdown => write!(f, "Shutdown"),
        }
    }
}

/// Payload for the `SessionCommand::Execute` variant.
/// Boxed to keep the enum size small.
struct SessionExecutePayload {
    agent: Arc<Agent>,
    message: IncomingMessage,
    mode: ExecutionMode,
    progress_cb: Option<ProgressCallback>,
    respond_to: oneshot::Sender<crate::Result<OutgoingMessage>>,
}

/// Internal command sent to a per-session actor
enum SessionCommand {
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
                )
                .await;

                let _ = handle
                    .tx
                    .send(SessionCommand::Execute(Box::new(SessionExecutePayload {
                        agent,
                        message,
                        mode: ExecutionMode::Session,
                        progress_cb: None,
                        respond_to,
                    })))
                    .await;
            }

            AcpCommand::ExecuteRun { agent, message, respond_to } => {
                let session_id = message.conversation_id.0.clone();
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Run,
                )
                .await;

                let _ = handle
                    .tx
                    .send(SessionCommand::Execute(Box::new(SessionExecutePayload {
                        agent,
                        message,
                        mode: ExecutionMode::Run,
                        progress_cb: None,
                        respond_to,
                    })))
                    .await;
            }

            AcpCommand::ExecuteSessionWithProgress {
                agent,
                message,
                progress_cb,
                respond_to,
            } => {
                let session_id = message.conversation_id.0.clone();
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Session,
                )
                .await;

                let _ = handle
                    .tx
                    .send(SessionCommand::Execute(Box::new(SessionExecutePayload {
                        agent,
                        message,
                        mode: ExecutionMode::Session,
                        progress_cb: Some(progress_cb),
                        respond_to,
                    })))
                    .await;
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
                for handle in sessions.values() {
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
                let max_iter = 50;

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

                let _ = payload.respond_to.send(result);
            }

            SessionCommand::Shutdown => {
                info!("Session actor shutting down for {}", session_id);
                break;
            }
        }
    }

    info!("Session actor ended for {}", session_id);
}

/// Subagent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentConfig {
    /// Agent type/personality to use
    pub agent_type: String,
    /// Spawn mode
    pub mode: SpawnMode,
    /// Thread binding
    pub thread_binding: ThreadBinding,
    /// System prompt override
    pub system_prompt: Option<String>,
    /// Maximum tokens
    pub max_tokens: Option<usize>,
    /// Temperature
    pub temperature: Option<f32>,
    /// Tools to enable
    pub tools: Vec<String>,
    /// Initial context/data
    pub context: Option<serde_json::Value>,
    /// Timeout in seconds (for Run mode)
    pub timeout_seconds: Option<u64>,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            agent_type: "default".to_string(),
            mode: SpawnMode::Run,
            thread_binding: ThreadBinding::Auto,
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            tools: vec![],
            context: None,
            timeout_seconds: Some(300),
        }
    }
}

/// Subagent handle - reference to a spawned subagent
#[derive(Debug, Clone)]
pub struct SubagentHandle {
    /// Subagent ID
    pub id: String,
    /// Parent agent ID
    pub parent_id: String,
    /// ACP Session ID
    pub session_id: AcpSessionId,
    /// Spawn mode
    pub mode: SpawnMode,
    /// Thread ID this agent is bound to
    pub thread_id: String,
    /// Command channel to subagent
    pub command_tx: mpsc::Sender<SubagentCommand>,
    /// Current status
    pub status: SubagentStatus,
    /// Execution controller for runtime pause/resume/step/cancel
    pub controller: Arc<ExecutionController>,
}

/// Subagent status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    /// Starting up
    Starting,
    /// Ready for work
    Ready,
    /// Busy processing
    Busy,
    /// Shutting down
    ShuttingDown,
    /// Terminated normally
    Terminated,
    /// Terminated due to a panic — detected by the watchdog task
    Crashed,
}

/// Commands that can be sent to a subagent
#[derive(Debug)]
pub enum SubagentCommand {
    /// Process a message
    ProcessMessage {
        message: IncomingMessage,
        response_tx: oneshot::Sender<crate::Result<String>>,
    },
    /// Update configuration
    UpdateConfig(AgentConfig),
    /// Cancel current operation
    Cancel,
    /// Shutdown the subagent
    Shutdown,
}

impl Clone for SubagentCommand {
    fn clone(&self) -> Self {
        match self {
            Self::UpdateConfig(config) => Self::UpdateConfig(config.clone()),
            Self::Cancel => Self::Cancel,
            Self::Shutdown => Self::Shutdown,
            // ProcessMessage can't be cloned due to oneshot, convert to Cancel
            Self::ProcessMessage { .. } => Self::Cancel,
        }
    }
}

/// Response from a subagent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResponse {
    pub subagent_id: String,
    pub result: Result<String, String>,
    pub metadata: Option<serde_json::Value>,
}

/// Thread context for serialized execution
#[derive(Debug)]
pub struct ThreadContext {
    /// Thread ID
    pub id: String,
    /// Active subagent on this thread (if any)
    pub active_subagent: Option<String>,
    /// Message queue for this thread
    pub queue: Vec<ThreadMessage>,
    /// Created timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Message in a thread queue
#[derive(Debug)]
pub struct ThreadMessage {
    pub id: String,
    pub subagent_id: String,
    pub message: IncomingMessage,
    pub response_tx: Option<oneshot::Sender<crate::Result<String>>>,
    pub queued_at: chrono::DateTime<chrono::Utc>,
}

/// ACP Control Plane - unified control plane for agents and subagents
pub struct AcpControlPlane {
    /// Subagents by ID
    subagents: Arc<RwLock<HashMap<String, SubagentHandle>>>,
    /// Threads by ID
    threads: Arc<RwLock<HashMap<String, ThreadContext>>>,
    /// ACP sessions
    sessions: Arc<RwLock<HashMap<AcpSessionId, AcpSession>>>,
    /// Default agent builder (set after initialization when provider/tools are ready)
    default_agent_builder: Arc<RwLock<Option<Arc<dyn Fn() -> crate::Result<Agent> + Send + Sync>>>>,
    /// Command channel to the ACP actor loop
    command_tx: mpsc::Sender<AcpCommand>,
}

/// ACP Session - groups related subagents
#[derive(Debug)]
pub struct AcpSession {
    pub id: AcpSessionId,
    pub parent_agent_id: String,
    pub subagents: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl AcpControlPlane {
    /// Create a new ACP control plane and spawn the actor loop
    pub fn new() -> Self {
        let (command_tx, command_rx) = mpsc::channel(256);
        tokio::spawn(acp_actor_loop(command_rx));
        Self {
            subagents: Arc::new(RwLock::new(HashMap::new())),
            threads: Arc::new(RwLock::new(HashMap::new())),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            default_agent_builder: Arc::new(RwLock::new(None)),
            command_tx,
        }
    }

    /// Set the default agent builder (consuming self).
    pub fn with_agent_builder<F>(self, builder: F) -> Self
    where
        F: Fn() -> crate::Result<Agent> + Send + Sync + 'static,
    {
        *self.default_agent_builder.blocking_write() = Some(Arc::new(builder));
        self
    }

    /// Set the default agent builder on an existing instance.
    ///
    /// Use this when the builder depends on resources created after the ACP.
    pub async fn set_agent_builder<F>(&self, builder: F)
    where
        F: Fn() -> crate::Result<Agent> + Send + Sync + 'static,
    {
        let mut guard = self.default_agent_builder.write().await;
        *guard = Some(Arc::new(builder));
        info!("ACP default agent builder configured");
    }

    // ------------------------------------------------------------------
    // Serial execution queue (inherited from legacy AcpController)
    // ------------------------------------------------------------------

    /// Execute a message in persistent session mode
    pub async fn execute_session(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
    ) -> crate::Result<OutgoingMessage> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(AcpCommand::ExecuteSession { agent, message, respond_to: tx })
            .await;
        rx.await
            .map_err(|_| crate::error::MantaError::Internal("ACP channel closed".to_string()))?
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
            .send(AcpCommand::ExecuteRun { agent, message, respond_to: tx })
            .await;
        rx.await
            .map_err(|_| crate::error::MantaError::Internal("ACP channel closed".to_string()))?
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
        rx.await
            .map_err(|_| crate::error::MantaError::Internal("ACP channel closed".to_string()))?
    }

    /// Pause a running session
    pub async fn pause(&self, session_id: String) {
        let _ = self.command_tx.send(AcpCommand::Pause { session_id }).await;
    }

    /// Resume a paused session
    pub async fn resume(&self, session_id: String) {
        let _ = self
            .command_tx
            .send(AcpCommand::Resume { session_id })
            .await;
    }

    /// Single step a paused session
    pub async fn step(&self, session_id: String) {
        let _ = self.command_tx.send(AcpCommand::Step { session_id }).await;
    }

    /// Cancel a running session
    pub async fn cancel(&self, session_id: String) {
        let _ = self
            .command_tx
            .send(AcpCommand::Cancel { session_id })
            .await;
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

    /// Shutdown the ACP actor loop
    pub async fn shutdown(&self) {
        let _ = self.command_tx.send(AcpCommand::Shutdown).await;
    }

    // ------------------------------------------------------------------
    // Subagent management
    // ------------------------------------------------------------------

    /// Create a new ACP session
    pub async fn create_session(&self, parent_agent_id: String) -> AcpSessionId {
        let session_id = AcpSessionId::new();
        let session = AcpSession {
            id: session_id.clone(),
            parent_agent_id,
            subagents: vec![],
            created_at: chrono::Utc::now(),
        };

        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.clone(), session);

        info!("Created ACP session {}", session_id);
        session_id
    }

    /// Spawn a subagent
    pub async fn spawn_subagent(
        &self,
        session_id: AcpSessionId,
        parent_id: String,
        config: SubagentConfig,
    ) -> crate::Result<SubagentHandle> {
        let subagent_id = format!("subagent-{}", Uuid::new_v4());
        let thread_id = self
            .resolve_thread_id(&config.thread_binding, &parent_id)
            .await;

        info!(
            "Spawning subagent {} (mode: {:?}, thread: {})",
            subagent_id, config.mode, thread_id
        );

        // Create command channel
        let (command_tx, mut command_rx) = mpsc::channel::<SubagentCommand>(100);

        // Build agent config
        let _agent_config = AgentConfig {
            system_prompt: config.system_prompt.unwrap_or_default(),
            max_tokens: config.max_tokens.map(|m| m as u32).unwrap_or(2048),
            max_context_tokens: 4096,
            max_concurrent_tools: 5,
            temperature: config.temperature.unwrap_or(0.7),
            skills_prompt: None,
            max_turns: None,
            compaction_model: None,
        };

        // Create the agent
        let agent = {
            let builder_guard = self.default_agent_builder.read().await;
            if let Some(ref builder) = *builder_guard {
                builder()?
            } else {
                return Err(crate::error::MantaError::Internal(
                    "No agent builder configured".to_string(),
                ));
            }
        };

        // Create execution controller for runtime pause/resume/step/cancel
        let controller = ExecutionController::new();
        let controller_clone = controller.clone();

        // Spawn subagent task
        let subagent_id_clone = subagent_id.clone();
        let mode = config.mode;
        let timeout = config.timeout_seconds;

        let join_handle = tokio::spawn(async move {
            info!("Subagent {} task started", subagent_id_clone);
            let mut agent = agent;

            while let Some(cmd) = command_rx.recv().await {
                match cmd {
                    SubagentCommand::ProcessMessage { message, response_tx } => {
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
                                    crate::agent::ProgressEvent::ToolResult { name, result } => {
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
                        let max_iter = 50;

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

                        let response = match result {
                            Ok(Ok(response)) => Ok(response.content),
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(_) => Err("Timeout".to_string()),
                        };

                        let _ =
                            response_tx.send(response.map_err(crate::error::MantaError::Internal));

                        // For Run mode, terminate after first message
                        if mode == SpawnMode::Run {
                            info!("Subagent {} (Run mode) completing", subagent_id_clone);
                            break;
                        }
                    }
                    SubagentCommand::UpdateConfig(new_config) => {
                        debug!("Subagent {} config updated", subagent_id_clone);
                        agent.update_config(new_config);
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

        // Watchdog: await the JoinHandle and update status on exit or panic.
        let subagents_ref = Arc::clone(&self.subagents);
        let watch_id = subagent_id.clone();
        tokio::spawn(async move {
            match join_handle.await {
                Ok(()) => {
                    let mut map = subagents_ref.write().await;
                    if let Some(h) = map.get_mut(&watch_id) {
                        h.status = SubagentStatus::Terminated;
                    }
                }
                Err(e) if e.is_panic() => {
                    warn!("Subagent {} panicked — marking Crashed", watch_id);
                    let mut map = subagents_ref.write().await;
                    if let Some(h) = map.get_mut(&watch_id) {
                        h.status = SubagentStatus::Crashed;
                    }
                }
                Err(_) => {
                    // Aborted externally
                    let mut map = subagents_ref.write().await;
                    if let Some(h) = map.get_mut(&watch_id) {
                        h.status = SubagentStatus::Terminated;
                    }
                }
            }
        });

        let handle = SubagentHandle {
            id: subagent_id.clone(),
            parent_id: parent_id.clone(),
            session_id: session_id.clone(),
            mode: config.mode,
            thread_id: thread_id.clone(),
            command_tx,
            status: SubagentStatus::Ready,
            controller: controller_clone,
        };

        // Register subagent
        let mut subagents = self.subagents.write().await;
        subagents.insert(subagent_id.clone(), handle.clone());

        // Register with session
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&session_id) {
            session.subagents.push(subagent_id.clone());
        }

        // Ensure thread exists
        let mut threads = self.threads.write().await;
        if !threads.contains_key(&thread_id) {
            threads.insert(
                thread_id.clone(),
                ThreadContext {
                    id: thread_id.clone(),
                    active_subagent: None,
                    queue: vec![],
                    created_at: chrono::Utc::now(),
                },
            );
        }

        info!("Subagent {} spawned successfully", subagent_id);
        Ok(handle)
    }

    /// Resolve thread ID based on binding mode
    async fn resolve_thread_id(&self, binding: &ThreadBinding, parent_id: &str) -> String {
        match binding {
            ThreadBinding::New => format!("thread-{}", Uuid::new_v4()),
            ThreadBinding::Parent => format!("thread-{}", parent_id),
            ThreadBinding::Thread(id) => id.clone(),
            ThreadBinding::Auto => {
                // Check if parent has a thread
                let subagents = self.subagents.read().await;
                if let Some(parent) = subagents.get(parent_id) {
                    parent.thread_id.clone()
                } else {
                    format!("thread-{}", parent_id)
                }
            }
        }
    }

    /// Send a message to a subagent
    pub async fn send_message(
        &self,
        subagent_id: &str,
        message: IncomingMessage,
    ) -> crate::Result<String> {
        let subagents = self.subagents.read().await;
        let subagent =
            subagents
                .get(subagent_id)
                .ok_or_else(|| crate::error::MantaError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                })?;

        let (response_tx, response_rx) = oneshot::channel();

        subagent
            .command_tx
            .send(SubagentCommand::ProcessMessage { message, response_tx })
            .await
            .map_err(|_| {
                crate::error::MantaError::Internal("Subagent command channel closed".to_string())
            })?;

        let result = response_rx.await.map_err(|_| {
            crate::error::MantaError::Internal("Subagent response channel closed".to_string())
        })??;

        Ok(result)
    }

    /// Shutdown a subagent
    pub async fn shutdown_subagent(&self, subagent_id: &str) -> crate::Result<bool> {
        let mut subagents = self.subagents.write().await;

        if let Some(subagent) = subagents.get_mut(subagent_id) {
            subagent.status = SubagentStatus::ShuttingDown;
            let _ = subagent.command_tx.send(SubagentCommand::Shutdown).await;
            // Watchdog task will update status to Terminated once the task exits.
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Terminate all subagents in a session
    pub async fn terminate_session(&self, session_id: &AcpSessionId) -> crate::Result<usize> {
        let sessions = self.sessions.read().await;
        let session =
            sessions
                .get(session_id)
                .ok_or_else(|| crate::error::MantaError::NotFound {
                    resource: format!("Session '{}'", session_id),
                })?;

        let subagent_ids: Vec<String> = session.subagents.clone();
        drop(sessions);

        let mut count = 0;
        for subagent_id in subagent_ids {
            if self.shutdown_subagent(&subagent_id).await? {
                count += 1;
            }
        }

        // Remove session
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);

        info!("Terminated {} subagents in session {}", count, session_id);
        Ok(count)
    }

    /// Get subagent status
    pub async fn get_subagent_status(&self, subagent_id: &str) -> Option<SubagentStatus> {
        let subagents = self.subagents.read().await;
        subagents.get(subagent_id).map(|s| s.status)
    }

    /// List all subagents
    pub async fn list_subagents(&self) -> Vec<SubagentHandle> {
        let subagents = self.subagents.read().await;
        subagents.values().cloned().collect()
    }

    /// List subagents in a session
    pub async fn list_session_subagents(&self, session_id: &AcpSessionId) -> Vec<SubagentHandle> {
        let sessions = self.sessions.read().await;
        let subagents = self.subagents.read().await;

        if let Some(session) = sessions.get(session_id) {
            session
                .subagents
                .iter()
                .filter_map(|id| subagents.get(id).cloned())
                .collect()
        } else {
            vec![]
        }
    }

    /// Get session info
    pub async fn get_session_info(&self, session_id: &AcpSessionId) -> Option<AcpSessionInfo> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|s| AcpSessionInfo {
            id: s.id.clone(),
            parent_agent_id: s.parent_agent_id.clone(),
            subagent_count: s.subagents.len(),
            created_at: s.created_at,
        })
    }

    /// Get subagent tree for a session (recursive parent-child hierarchy)
    pub async fn get_subagent_tree(&self, session_id: &AcpSessionId) -> Vec<SubagentTreeNode> {
        let sessions = self.sessions.read().await;
        let subagents = self.subagents.read().await;

        let session = match sessions.get(session_id) {
            Some(s) => s,
            None => return vec![],
        };

        let root_parent_id = session.parent_agent_id.clone();
        let session_subagent_ids = session.subagents.clone();
        drop(sessions);

        let mut by_parent: HashMap<String, Vec<SubagentHandle>> = HashMap::new();
        let mut all_ids = std::collections::HashSet::new();

        for id in session_subagent_ids {
            if let Some(subagent) = subagents.get(&id) {
                all_ids.insert(subagent.id.clone());
                by_parent
                    .entry(subagent.parent_id.clone())
                    .or_default()
                    .push(subagent.clone());
            }
        }
        drop(subagents);

        fn build_tree(
            parent_id: &str,
            by_parent: &HashMap<String, Vec<SubagentHandle>>,
            all_ids: &std::collections::HashSet<String>,
        ) -> Vec<SubagentTreeNode> {
            by_parent
                .get(parent_id)
                .map(|children| {
                    children
                        .iter()
                        .map(|s| SubagentTreeNode {
                            id: s.id.clone(),
                            parent_id: s.parent_id.clone(),
                            status: s.status,
                            mode: s.mode,
                            thread_id: s.thread_id.clone(),
                            children: if all_ids.contains(&s.id) {
                                build_tree(&s.id, by_parent, all_ids)
                            } else {
                                vec![]
                            },
                        })
                        .collect()
                })
                .unwrap_or_default()
        }

        build_tree(&root_parent_id, &by_parent, &all_ids)
    }
}

impl Default for AcpControlPlane {
    fn default() -> Self {
        Self::new()
    }
}

/// Session info for display
#[derive(Debug, Clone)]
pub struct AcpSessionInfo {
    pub id: AcpSessionId,
    pub parent_agent_id: String,
    pub subagent_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Subagent tree node for hierarchical display
#[derive(Debug, Clone, Serialize)]
pub struct SubagentTreeNode {
    pub id: String,
    pub parent_id: String,
    pub status: SubagentStatus,
    pub mode: SpawnMode,
    pub thread_id: String,
    pub children: Vec<SubagentTreeNode>,
}

/// Extension trait for Agent to support ACP
#[async_trait]
pub trait AcpAgentExt {
    /// Spawn a subagent from this agent
    async fn spawn_subagent(
        &self,
        acp: &AcpControlPlane,
        config: SubagentConfig,
    ) -> crate::Result<SubagentHandle>;
}

#[async_trait]
impl AcpAgentExt for AgentHandle {
    async fn spawn_subagent(
        &self,
        acp: &AcpControlPlane,
        config: SubagentConfig,
    ) -> crate::Result<SubagentHandle> {
        let session_id = AcpSessionId::new();
        acp.spawn_subagent(session_id, self.id.clone(), config)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acp_session_id_new() {
        let id1 = AcpSessionId::new();
        let id2 = AcpSessionId::new();
        assert_ne!(id1.0, id2.0);
        assert!(!id1.0.is_empty());
    }

    #[test]
    fn test_acp_session_id_default() {
        let id = AcpSessionId::default();
        assert!(!id.0.is_empty());
    }

    #[test]
    fn test_acp_session_id_display() {
        let id = AcpSessionId("sess-123".to_string());
        assert_eq!(format!("{}", id), "sess-123");
    }

    #[test]
    fn test_spawn_mode_default() {
        assert_eq!(SpawnMode::default(), SpawnMode::Run);
    }

    #[test]
    fn test_spawn_mode_serde() {
        let run = serde_json::to_value(SpawnMode::Run).unwrap();
        assert_eq!(run, "run");
        let session = serde_json::to_value(SpawnMode::Session).unwrap();
        assert_eq!(session, "session");

        let decoded: SpawnMode = serde_json::from_str("\"session\"").unwrap();
        assert_eq!(decoded, SpawnMode::Session);
    }

    #[test]
    fn test_thread_binding_default() {
        assert!(matches!(ThreadBinding::default(), ThreadBinding::Auto));
    }

    #[test]
    fn test_thread_binding_serde() {
        let new = serde_json::to_value(ThreadBinding::New).unwrap();
        assert_eq!(new, "new");
        let parent = serde_json::to_value(ThreadBinding::Parent).unwrap();
        assert_eq!(parent, "parent");
        let auto = serde_json::to_value(ThreadBinding::Auto).unwrap();
        assert_eq!(auto, "auto");

        let decoded: ThreadBinding = serde_json::from_str("\"auto\"").unwrap();
        assert!(matches!(decoded, ThreadBinding::Auto));
    }

    #[test]
    fn test_subagent_config_default() {
        let config = SubagentConfig::default();
        assert_eq!(config.agent_type, "default");
        assert_eq!(config.mode, SpawnMode::Run);
        assert!(matches!(config.thread_binding, ThreadBinding::Auto));
        assert!(config.system_prompt.is_none());
        assert!(config.max_tokens.is_none());
        assert!(config.temperature.is_none());
        assert!(config.tools.is_empty());
        assert!(config.context.is_none());
        assert_eq!(config.timeout_seconds, Some(300));
    }

    #[test]
    fn test_subagent_status_serde() {
        let status = serde_json::to_value(SubagentStatus::Ready).unwrap();
        assert_eq!(status, "ready");
        let status = serde_json::to_value(SubagentStatus::Crashed).unwrap();
        assert_eq!(status, "crashed");
    }

    #[tokio::test]
    async fn test_acp_control_plane_new() {
        let acp = AcpControlPlane::new();
        let subagents = acp.list_subagents().await;
        assert!(subagents.is_empty());
    }

    #[tokio::test]
    async fn test_create_session() {
        let acp = AcpControlPlane::new();
        let session_id = acp.create_session("parent-1".to_string()).await;
        assert!(!session_id.0.is_empty());

        let info = acp.get_session_info(&session_id).await;
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.parent_agent_id, "parent-1");
        assert_eq!(info.subagent_count, 0);
    }

    #[tokio::test]
    async fn test_get_session_info_not_found() {
        let acp = AcpControlPlane::new();
        let info = acp
            .get_session_info(&AcpSessionId("nonexistent".to_string()))
            .await;
        assert!(info.is_none());
    }

    #[tokio::test]
    async fn test_terminate_session_not_found() {
        let acp = AcpControlPlane::new();
        let result = acp
            .terminate_session(&AcpSessionId("nonexistent".to_string()))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_session_subagents_empty() {
        let acp = AcpControlPlane::new();
        let session_id = acp.create_session("parent".to_string()).await;
        let subagents = acp.list_session_subagents(&session_id).await;
        assert!(subagents.is_empty());
    }

    #[tokio::test]
    async fn test_get_subagent_status_not_found() {
        let acp = AcpControlPlane::new();
        let status = acp.get_subagent_status("nonexistent").await;
        assert!(status.is_none());
    }

    #[tokio::test]
    async fn test_resolve_thread_id_new() {
        let acp = AcpControlPlane::new();
        let id = acp.resolve_thread_id(&ThreadBinding::New, "parent").await;
        assert!(id.starts_with("thread-"));
    }

    #[tokio::test]
    async fn test_resolve_thread_id_parent() {
        let acp = AcpControlPlane::new();
        let id = acp
            .resolve_thread_id(&ThreadBinding::Parent, "parent-1")
            .await;
        assert!(id.contains("parent-1"));
    }

    #[tokio::test]
    async fn test_resolve_thread_id_specific() {
        let acp = AcpControlPlane::new();
        let id = acp
            .resolve_thread_id(&ThreadBinding::Thread("my-thread".to_string()), "parent")
            .await;
        assert_eq!(id, "my-thread");
    }

    #[tokio::test]
    async fn test_resolve_thread_id_auto() {
        let acp = AcpControlPlane::new();
        let id = acp
            .resolve_thread_id(&ThreadBinding::Auto, "parent-1")
            .await;
        assert!(id.contains("parent-1"));
    }

    #[tokio::test]
    async fn test_execution_controller_running() {
        let ctrl = ExecutionController::new();
        ctrl.reset().await;
        // Running / Idle -> returns immediately
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
        // First call: Stepping -> returns, then becomes Paused
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
        let handle = tokio::spawn(async move { ctrl2.check_and_wait().await });

        // Small delay to let the task reach the wait
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Resume
        ctrl.resume().await;

        assert!(handle.await.unwrap().is_ok());
    }
}
