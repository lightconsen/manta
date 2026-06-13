//! Agent Control Plane (ACP) - Subagent Spawning System
//!
//! This provides:
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

use crate::agent::session_store::SaveSubagentRunParams;
use crate::agent::{Agent, AgentConfig, ProgressCallback};
use crate::channels::{IncomingMessage, OutgoingMessage};

// AgentHandle is defined in gateway module
pub use crate::gateway::AgentHandle;

/// Configuration for automatic crash recovery of subagents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrashRecoveryConfig {
    /// Whether to automatically restart crashed subagents.
    pub enabled: bool,
    /// Maximum number of restart attempts for a single subagent.
    pub max_retries: u32,
    /// Backoff delays in seconds between restart attempts.
    pub backoff_seconds: &'static [u64],
}

impl Default for CrashRecoveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: 3,
            backoff_seconds: &[1, 2, 5, 10, 30],
        }
    }
}

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
#[derive(Default)]
pub enum SpawnMode {
 /// One-shot execution (run and terminate)
    #[default]
    Run,
 /// Persistent session (long-running)
    Session,
}

/// Thread binding mode for subagents
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ThreadBinding {
 /// New isolated thread
    New,
 /// Bind to parent's thread
    Parent,
 /// Bind to specific thread ID
    Thread(String),
 /// Automatic based on context
    #[default]
    Auto,
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
    iteration: std::sync::atomic::AtomicUsize,
}

impl ExecutionController {
 /// Create a new controller in the `Idle` state.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: RwLock::new(RuntimeState::Idle),
            notify: tokio::sync::Notify::new(),
            iteration: std::sync::atomic::AtomicUsize::new(0),
        })
    }

 /// Check if execution should proceed.
    pub async fn check_and_wait(&self) -> Result<(), &'static str> {
        loop {
            let state = *self.state.read().await;
            match state {
                RuntimeState::Idle | RuntimeState::Running => {
                    self.iteration
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    return Ok(());
                }
                RuntimeState::Stepping => {
                    self.iteration
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        self.iteration.store(0, std::sync::atomic::Ordering::SeqCst);
    }

 /// Current runtime state.
    pub async fn current_state(&self) -> RuntimeState {
        *self.state.read().await
    }

 /// Current iteration count (number of times check_and_wait has allowed
 /// execution to proceed).
    pub fn current_iteration(&self) -> usize {
        self.iteration.load(std::sync::atomic::Ordering::SeqCst)
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
async fn acp_actor_loop(mut command_rx: mpsc::Receiver<AcpCommand>, max_iterations: usize) {
    let mut sessions: HashMap<String, SessionHandle> = HashMap::new();
    let mut session_meta: HashMap<String, SessionExecution> = HashMap::new();

    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            AcpCommand::ExecuteSession {
                agent,
                message,
                max_iterations: req_max_iter,
                respond_to,
            } => {
                let session_id = message.conversation_id.0.clone();
                let effective_max = req_max_iter.unwrap_or(max_iterations);
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Session,
                    effective_max,
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

            AcpCommand::ExecuteRun {
                agent,
                message,
                max_iterations: req_max_iter,
                respond_to,
            } => {
                let session_id = message.conversation_id.0.clone();
                let effective_max = req_max_iter.unwrap_or(max_iterations);
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Run,
                    effective_max,
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
                max_iterations: req_max_iter,
                respond_to,
            } => {
                let session_id = message.conversation_id.0.clone();
                let effective_max = req_max_iter.unwrap_or(max_iterations);
                let handle = get_or_create_session(
                    &mut sessions,
                    &mut session_meta,
                    &session_id,
                    ExecutionMode::Session,
                    effective_max,
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
                        current_iteration: handle.controller.current_iteration(),
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
    max_iterations: usize,
) -> &'a SessionHandle {
    if !sessions.contains_key(session_id) {
        let (tx, rx) = mpsc::channel::<SessionCommand>(256);
        let controller = ExecutionController::new();
        let ctrl_clone = controller.clone();

        let meta = SessionExecution {
            controller: controller.clone(),
            mode,
            current_iteration: 0,
            max_iterations,
            current_message: None,
        };

        let handle = SessionHandle {
            tx,
            controller: controller.clone(),
            mode,
        };

        tokio::spawn(session_actor_loop(rx, ctrl_clone, session_id.to_string(), max_iterations));

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
    max_iterations: usize,
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
                let max_iter = max_iterations;

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
 /// Automatically restart if the subagent crashes (default: false)
    #[serde(default)]
    pub retry_on_crash: bool,
 /// Maximum number of crash restart attempts (default: 3)
    #[serde(default = "default_max_crash_retries")]
    pub max_crash_retries: u32,
}

fn default_max_crash_retries() -> u32 {
    3
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
            retry_on_crash: false,
            max_crash_retries: default_max_crash_retries(),
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
 /// Abort handle for force-killing the subagent task
    pub abort_handle: tokio::task::AbortHandle,
 /// Number of times this subagent has been restarted after a crash
    pub crash_count: u32,
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

/// Lightweight snapshot of a thread context that can be cloned and returned
/// to callers without exposing internal oneshot channels.
#[derive(Debug, Clone)]
pub struct ThreadContextSummary {
    /// Thread ID
    pub id: String,
    /// Active subagent on this thread (if any)
    pub active_subagent: Option<String>,
    /// Number of queued thread messages
    pub queue_len: usize,
    /// Created timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Message sent over the cross-session subagent bus.
#[derive(Debug, Clone)]
pub struct BusMessage {
    /// Unique message ID
    pub id: String,
    /// Topic the message was published on
    pub topic: String,
    /// Subagent ID that published the message
    pub sender_id: String,
    /// Message payload
    pub payload: String,
    /// When the message was sent
    pub sent_at: chrono::DateTime<chrono::Utc>,
}

/// Cross-session message bus for subagents.
///
/// Allows subagents in unrelated ACP sessions to communicate via named topics.
#[derive(Debug, Default, Clone)]
pub struct AcpBus {
    /// Messages per topic, oldest first.
    messages: HashMap<String, Vec<BusMessage>>,
    /// Topic subscriptions: topic -> set of subagent IDs.
    subscriptions: HashMap<String, std::collections::HashSet<String>>,
    /// Per-subagent per-topic read offsets.
    read_offsets: HashMap<(String, String), usize>,
}

impl AcpBus {
    /// Create an empty bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe a subagent to a topic.
    ///
    /// The subscriber starts receiving messages published after the
    /// subscription is created.
    pub fn subscribe(&mut self, subagent_id: &str, topic: &str) {
        self.subscriptions
            .entry(topic.to_string())
            .or_default()
            .insert(subagent_id.to_string());
        let current_len = self.messages.get(topic).map(|v| v.len()).unwrap_or(0);
        self.read_offsets
            .entry((subagent_id.to_string(), topic.to_string()))
            .or_insert(current_len);
    }

    /// Unsubscribe a subagent from a topic.
    pub fn unsubscribe(&mut self, subagent_id: &str, topic: &str) {
        if let Some(set) = self.subscriptions.get_mut(topic) {
            set.remove(subagent_id);
            if set.is_empty() {
                self.subscriptions.remove(topic);
            }
        }
        self.read_offsets
            .remove(&(subagent_id.to_string(), topic.to_string()));
    }

    /// Publish a message to a topic.
    pub fn publish(&mut self, topic: &str, sender_id: &str, payload: &str) -> BusMessage {
        let message = BusMessage {
            id: Uuid::new_v4().to_string(),
            topic: topic.to_string(),
            sender_id: sender_id.to_string(),
            payload: payload.to_string(),
            sent_at: chrono::Utc::now(),
        };
        self.messages
            .entry(topic.to_string())
            .or_default()
            .push(message.clone());
        message
    }

    /// Poll pending messages for a subagent on a topic.
    pub fn poll(&mut self, subagent_id: &str, topic: &str) -> Vec<BusMessage> {
        if !self
            .subscriptions
            .get(topic)
            .map(|s| s.contains(subagent_id))
            .unwrap_or(false)
        {
            return Vec::new();
        }
        let offset = self
            .read_offsets
            .entry((subagent_id.to_string(), topic.to_string()))
            .or_insert(0);
        let messages = self.messages.get(topic).map(|v| v.as_slice()).unwrap_or(&[]);
        let pending: Vec<BusMessage> = messages[*offset..].to_vec();
        *offset = messages.len();
        pending
    }

    /// Poll pending messages for a subagent across all subscribed topics.
    pub fn poll_all(&mut self, subagent_id: &str) -> HashMap<String, Vec<BusMessage>> {
        let topics: Vec<String> = self
            .subscriptions
            .iter()
            .filter(|(_, subs)| subs.contains(subagent_id))
            .map(|(topic, _)| topic.clone())
            .collect();
        topics
            .into_iter()
            .map(|topic| {
                let messages = self.poll(subagent_id, &topic);
                (topic, messages)
            })
            .collect()
    }

    /// List all topics that have at least one message or subscriber.
    pub fn topics(&self) -> Vec<String> {
        let mut topics: std::collections::HashSet<String> = self.messages.keys().cloned().collect();
        topics.extend(self.subscriptions.keys().cloned());
        topics.into_iter().collect()
    }

    /// List subscribers for a topic.
    pub fn subscribers(&self, topic: &str) -> Vec<String> {
        self.subscriptions
            .get(topic)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// ACP Control Plane - unified control plane for agents and subagents
#[derive(Clone)]
pub struct AcpControlPlane {
 /// Subagents by ID
    subagents: Arc<RwLock<HashMap<String, SubagentHandle>>>,
 /// Threads by ID
    threads: Arc<RwLock<HashMap<String, ThreadContext>>>,
 /// ACP sessions
    sessions: Arc<RwLock<HashMap<AcpSessionId, AcpSession>>>,
 /// Default agent builder (set after initialization when provider/tools are ready)
    #[allow(clippy::type_complexity)]
    default_agent_builder: Arc<RwLock<Option<Arc<dyn Fn() -> crate::Result<Agent> + Send + Sync>>>>,
 /// Command channel to the ACP actor loop
    command_tx: mpsc::Sender<AcpCommand>,
 /// Optional session store for persisting subagent run records
    store: Option<Arc<crate::agent::session_store::SessionStore>>,
 /// Maximum iterations per ACP execution
    max_iterations: usize,
 /// Configuration controlling automatic crash recovery.
    recovery: Arc<RwLock<CrashRecoveryConfig>>,
    /// Cross-session subagent communication bus.
    bus: Arc<RwLock<AcpBus>>,
    /// Event broadcast channel for ACP lifecycle events.
    event_tx: Arc<RwLock<Option<tokio::sync::broadcast::Sender<crate::gateway::GatewayEvent>>>>,
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
    pub fn new(max_iterations: usize) -> Self {
        let (command_tx, command_rx) = mpsc::channel(256);
        tokio::spawn(acp_actor_loop(command_rx, max_iterations));
        Self {
            subagents: Arc::new(RwLock::new(HashMap::new())),
            threads: Arc::new(RwLock::new(HashMap::new())),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            default_agent_builder: Arc::new(RwLock::new(None)),
            command_tx,
            store: None,
            max_iterations,
            recovery: Arc::new(RwLock::new(CrashRecoveryConfig::default())),
            bus: Arc::new(RwLock::new(AcpBus::new())),
            event_tx: Arc::new(RwLock::new(None)),
        }
    }

 /// Attach a session store for persisting subagent run records.
    pub fn with_store(mut self, store: Arc<crate::agent::session_store::SessionStore>) -> Self {
        self.store = Some(store);
        self
    }

 /// Set the default agent builder (consuming self).
    pub fn with_agent_builder<F>(self, builder: F) -> Self
    where
        F: Fn() -> crate::Result<Agent> + Send + Sync + 'static,
    {
        {
            let mut guard = self.default_agent_builder.try_write()
                .expect("agent builder lock available during construction");
            *guard = Some(Arc::new(builder));
        }
        self
    }

 /// Configure automatic crash recovery.
    pub fn with_recovery(self, recovery: CrashRecoveryConfig) -> Self {
        {
            let mut guard = self.recovery.try_write()
                .expect("recovery lock available during construction");
            *guard = recovery;
        }
        self
    }

 /// Update crash recovery configuration at runtime.
    pub async fn set_recovery_config(&self, recovery: CrashRecoveryConfig) {
        let mut guard = self.recovery.write().await;
        *guard = recovery;
        info!("ACP crash recovery config updated: enabled={}", recovery.enabled);
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

 /// Set the event broadcast sender on an existing instance.
 ///
 /// Use this when the ACP is created before the GatewayState that owns the
 /// event broadcast channel.
    pub async fn set_event_tx(
        &self,
        event_tx: tokio::sync::broadcast::Sender<crate::gateway::GatewayEvent>,
    ) {
        let mut guard = self.event_tx.write().await;
        *guard = Some(event_tx);
        info!("ACP event broadcaster configured");
    }

    /// Emit an ACP lifecycle event if the broadcast channel is configured.
    async fn emit(&self, event: crate::gateway::GatewayEvent) {
        let guard = self.event_tx.read().await;
        if let Some(ref tx) = *guard {
            let _ = tx.send(event);
        }
    }

 /// Returns true if a session store is attached for persistence.
    pub fn has_store(&self) -> bool {
        self.store.is_some()
    }

 /// Load persisted ACP sessions from the store into memory.
    pub async fn load_persisted_sessions(&self) {
        let Some(ref store) = self.store else {
            info!("ACP session store not configured; skipping load_persisted_sessions");
            return;
        };

        match store.list_acp_sessions().await {
            Ok(rows) => {
                let mut loaded = 0usize;
                let mut sessions = self.sessions.write().await;
                for (session_id, parent_id, subagent_ids, created_at) in rows {
                    if session_id.is_empty() || parent_id.is_empty() {
                        warn!(
                            "Skipping malformed persisted ACP session (session_id={}, parent_id={})",
                            session_id, parent_id
                        );
                        continue;
                    }
                    sessions.insert(
                        AcpSessionId(session_id.clone()),
                        AcpSession {
                            id: AcpSessionId(session_id),
                            parent_agent_id: parent_id,
                            subagents: subagent_ids,
                            created_at,
                        },
                    );
                    loaded += 1;
                }
                info!("Loaded {} persisted ACP session(s)", loaded);
            }
            Err(e) => {
                warn!("Failed to load persisted ACP sessions: {}", e);
            }
        }
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
        self.execute_session_with_max_iterations(agent, message, None)
            .await
    }

 /// Execute a message in persistent session mode with optional max iteration override.
    pub async fn execute_session_with_max_iterations(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
        max_iterations: Option<usize>,
    ) -> crate::Result<OutgoingMessage> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(AcpCommand::ExecuteSession {
                agent,
                message,
                max_iterations,
                respond_to: tx,
            })
            .await;
        rx.await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?
    }

 /// Execute a message in one-shot run mode
    pub async fn execute_run(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
    ) -> crate::Result<OutgoingMessage> {
        self.execute_run_with_max_iterations(agent, message, None)
            .await
    }

 /// Execute a message in one-shot run mode with optional max iteration override.
    pub async fn execute_run_with_max_iterations(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
        max_iterations: Option<usize>,
    ) -> crate::Result<OutgoingMessage> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(AcpCommand::ExecuteRun {
                agent,
                message,
                max_iterations,
                respond_to: tx,
            })
            .await;
        rx.await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?
    }

 /// Execute with progress callbacks in session mode
    pub async fn execute_session_with_progress(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
    ) -> crate::Result<OutgoingMessage> {
        self.execute_session_with_progress_and_max_iterations(agent, message, progress_cb, None)
            .await
    }

 /// Execute with progress callbacks in session mode with optional max iteration override.
    pub async fn execute_session_with_progress_and_max_iterations(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
        max_iterations: Option<usize>,
    ) -> crate::Result<OutgoingMessage> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(AcpCommand::ExecuteSessionWithProgress {
                agent,
                message,
                progress_cb,
                max_iterations,
                respond_to: tx,
            })
            .await;
        rx.await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?
    }

 /// Pause a running session
    pub async fn pause(&self, session_id: String) {
        let _ = self.command_tx.send(AcpCommand::Pause { session_id: session_id.clone() }).await;
        self.emit(crate::gateway::GatewayEvent::AcpStatusChanged {
            session_id,
            runtime_state: "paused".to_string(),
        })
        .await;
    }

 /// Resume a paused session
    pub async fn resume(&self, session_id: String) {
        let _ = self
            .command_tx
            .send(AcpCommand::Resume { session_id: session_id.clone() })
            .await;
        self.emit(crate::gateway::GatewayEvent::AcpStatusChanged {
            session_id,
            runtime_state: "running".to_string(),
        })
        .await;
    }

 /// Single step a paused session
    pub async fn step(&self, session_id: String) {
        let _ = self.command_tx.send(AcpCommand::Step { session_id: session_id.clone() }).await;
        self.emit(crate::gateway::GatewayEvent::AcpStatusChanged {
            session_id,
            runtime_state: "stepping".to_string(),
        })
        .await;
    }

 /// Cancel a running session
    pub async fn cancel(&self, session_id: String) {
        let _ = self
            .command_tx
            .send(AcpCommand::Cancel { session_id: session_id.clone() })
            .await;
        self.emit(crate::gateway::GatewayEvent::AcpStatusChanged {
            session_id,
            runtime_state: "cancelled".to_string(),
        })
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
            parent_agent_id: parent_agent_id.clone(),
            subagents: vec![],
            created_at: chrono::Utc::now(),
        };

        let mut sessions = self.sessions.write().await;
        sessions.insert(session_id.clone(), session);
        drop(sessions);

 // Persist session if store is available
        if let Some(ref store) = self.store {
            let _ = store
                .save_acp_session(&session_id.0, &parent_agent_id, &[], chrono::Utc::now())
                .await;
        }

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

 // Resolve thread ID (acquire guard, read, release)
        let thread_id = {
            let threads = self.threads.read().await;
            match &config.thread_binding {
                ThreadBinding::New => format!("thread-{}", Uuid::new_v4()),
                ThreadBinding::Parent => format!("thread-{}", parent_id),
                ThreadBinding::Thread(id) => id.clone(),
                ThreadBinding::Auto => {
                    let candidate = format!("thread-{}", parent_id);
                    if threads.contains_key(&candidate) || threads.contains_key(&parent_id) {
                        threads
                            .get(&parent_id)
                            .map(|t| t.id.clone())
                            .unwrap_or_else(|| candidate.clone())
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

 // Build agent config
        let _agent_config = AgentConfig {
            system_prompt: config.system_prompt.clone().unwrap_or_default(),
            max_tokens: config.max_tokens.map(|m| m as u32).unwrap_or(2048),
            max_context_tokens: 4096,
            max_concurrent_tools: 5,
            temperature: config.temperature.unwrap_or(0.7),
            skills_prompt: None,
            max_turns: None,
            compaction_model: None,
            workspace_dir: None,
            workspace_only: false,
            heartbeat: None,
            agent_id: None,
        };

 // Create the agent (acquire builder, call, release)
        let agent = {
            let builder_guard = self.default_agent_builder.read().await;
            let result = if let Some(ref builder) = *builder_guard {
                builder()
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

 // Capture fields needed for crash recovery logging
        let recovery_retry_on_crash = config.retry_on_crash;
        let recovery_max_retries = config.max_crash_retries;
        let _recovery_config = self.recovery.clone();
        let acp_for_recovery = self.clone();
        let recovery_session_id = session_id.clone();
        let recovery_parent_id = parent_id.clone();
        let recovery_config_clone = config.clone();

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
                                    crate::agent::ProgressEvent::ToolResult { name, result, data: _ } => {
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

                        let response = match result {
                            Ok(Ok(response)) => Ok(response.content),
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(_) => Err("Timeout".to_string()),
                        };

                        let _ = response_tx
                            .send(response.map_err(crate::error::SyscityError::Internal));

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

        let abort_handle = join_handle.abort_handle();

 // Watchdog: await the JoinHandle and update status on exit or panic.
        let watchdog_subagents_ref = Arc::clone(&self.subagents);
        let watch_id = subagent_id.clone();
        let store_ref = self.store.clone();
        let acp_for_events = self.clone();
        let event_session_id = session_id.clone();
        tokio::spawn(async move {
            match join_handle.await {
                Ok(()) => {
                    let mut map = watchdog_subagents_ref.write().await;
                    if let Some(h) = map.get_mut(&watch_id) {
                        h.status = SubagentStatus::Terminated;
                    }
                    drop(map);
                    acp_for_events
                        .emit(crate::gateway::GatewayEvent::AcpCompleted {
                            session_id: event_session_id.to_string(),
                            subagent_id: watch_id.clone(),
                            status: "terminated".to_string(),
                        })
                        .await;
                    if let Some(store) = store_ref {
                        let _ = store
                            .complete_subagent_run(&watch_id, Some("normal exit"), None)
                            .await;
                    }
                }
                Err(e) if e.is_panic() => {
                    warn!("Subagent {} panicked — marking Crashed", watch_id);
                    let current_crash_count = {
                        let map = watchdog_subagents_ref.read().await;
                        map.get(&watch_id).map(|h| h.crash_count).unwrap_or(0)
                    };
                    {
                        let mut map = watchdog_subagents_ref.write().await;
                        if let Some(h) = map.get_mut(&watch_id) {
                            h.status = SubagentStatus::Crashed;
                        }
                    }
                    acp_for_events
                        .emit(crate::gateway::GatewayEvent::AcpCompleted {
                            session_id: event_session_id.to_string(),
                            subagent_id: watch_id.clone(),
                            status: "crashed".to_string(),
                        })
                        .await;
                    if let Some(store) = store_ref {
                        let _ = store
                            .complete_subagent_run(&watch_id, None, Some("panicked"))
                            .await;
                    }
 // Log crash for external recovery (call recover_crashed_subagent to restart)
                    if recovery_retry_on_crash && current_crash_count < recovery_max_retries {
                        warn!(
                            "Subagent {} crashed (attempt {}/{}). Auto-recovery enabled — call acp.recover_crashed_subagent() to restart.",
                            watch_id,
                            current_crash_count + 1,
                            recovery_max_retries
                        );
                    }

                    // Automatic recovery: if enabled globally or per-subagent, restart with backoff.
                    // The recovery future is not Send, so run it entirely inside a blocking task
                    // on the current runtime thread instead of spawning it back onto the executor.
                    let acp = acp_for_recovery.clone();
                    let sid = recovery_session_id.clone();
                    let pid = recovery_parent_id.clone();
                    let cfg = recovery_config_clone.clone();
                    tokio::task::spawn_blocking(move || {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(async {
                            let global = *acp.recovery.read().await;
                            let should_recover = (recovery_retry_on_crash || global.enabled)
                                && current_crash_count < recovery_max_retries;
                            if !should_recover {
                                return;
                            }
                            let delay = global
                                .backoff_seconds
                                .get(current_crash_count as usize)
                                .copied()
                                .unwrap_or_else(|| {
                                    global.backoff_seconds.last().copied().unwrap_or(30)
                                });
                            warn!(
                                "Auto-recovering subagent {} (attempt {}/{}) in {}s",
                                watch_id,
                                current_crash_count + 1,
                                recovery_max_retries,
                                delay
                            );
                            tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                            match acp
                                .recover_crashed_subagent(sid, pid, cfg, current_crash_count + 1)
                                .await
                            {
                                Some(new_handle) => {
                                    info!("Subagent {} recovered as {}", watch_id, new_handle.id);
                                }
                                None => {
                                    warn!("Failed to auto-recover subagent {}", watch_id);
                                }
                            }
                        });
                    });
                }
                Err(_) => {
 // Aborted externally
                    let mut map = watchdog_subagents_ref.write().await;
                    if let Some(h) = map.get_mut(&watch_id) {
                        h.status = SubagentStatus::Terminated;
                    }
                    drop(map);
                    acp_for_events
                        .emit(crate::gateway::GatewayEvent::AcpCompleted {
                            session_id: event_session_id.to_string(),
                            subagent_id: watch_id.clone(),
                            status: "aborted".to_string(),
                        })
                        .await;
 // Kill/abort already updates the store, so no-op here.
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
            abort_handle,
            crash_count: 0,
        };

 // Register subagent
        let mut subagents = self.subagents.write().await;
        subagents.insert(subagent_id.clone(), handle.clone());

 // Register with session
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
                let _ = store.save_acp_session(&sid, &parent, &ids, created).await;
            }
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

 // Persist subagent run record if store is attached.
        if let Some(ref store) = self.store {
            let _ = store
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
                .await;
            let _ = store
                .update_subagent_run_status(&subagent_id, "ready")
                .await;
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
    /// watchdog. It applies backoff based on `crash_count`, sets the crash
    /// counter on the new handle to the supplied value, persists a recovery
    /// event if a store is attached, and updates the session's subagent list
    /// to point to the replacement.
    pub async fn recover_crashed_subagent(
        &self,
        session_id: AcpSessionId,
        parent_id: String,
        config: SubagentConfig,
        crash_count: u32,
    ) -> Option<SubagentHandle> {
        let backoff_delays: &[u64] = {
            let guard = self.recovery.read().await;
            guard.backoff_seconds
        };
        let delay_idx = (crash_count as usize).min(backoff_delays.len().saturating_sub(1));
        let delay = backoff_delays.get(delay_idx).copied().unwrap_or(30);

        warn!(
            "Recovering crashed subagent (attempt {}, retrying in {}s)",
            crash_count + 1,
            delay
        );

        tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;

        match self.spawn_subagent(session_id.clone(), parent_id.clone(), config).await {
            Ok(handle) => {
                let new_id = handle.id.clone();
                let old_id = {
                    let subagents = self.subagents.read().await;
                    // Find the predecessor that was previously registered with the same
                    // configuration and session. The simplest heuristic is the first
                    // crashed subagent in the same session with a matching parent.
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
                            let _ = store
                                .save_acp_session(&sid, &parent, &ids, created)
                                .await;
                        }
                    }
                    {
                        let mut subagents = self.subagents.write().await;
                        subagents.remove(old_id);
                    }
                }

                if let Some(ref store) = self.store {
                    let _ = store
                        .save_subagent_run(
                            &SaveSubagentRunParams {
                                run_id: &new_id,
                                subagent_id: &new_id,
                                session_id: &session_id.to_string(),
                                parent_id: &handle.parent_id,
                                label: Some("recovery"),
                                task_prompt: Some(
                                    &format!(
                                        "auto-recovery after crash (attempt {})",
                                        crash_count + 1
                                    )
                                ),
                                mode: if handle.mode == SpawnMode::Run { "run" } else { "session" },
                                thread_id: Some(&handle.thread_id),
                            }
                        )
                        .await;
                    let _ = store.update_subagent_run_status(&new_id, "recovered").await;
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
            Err(e) => {
                warn!("Failed to recover crashed subagent: {}", e);
                None
            }
        }
    }

    // ------------------------------------------------------------------
    // Thread management
    // ------------------------------------------------------------------

    /// Ensure a thread context exists in the control plane.
    pub async fn ensure_thread(&self, thread_id: &str) {
        let mut threads = self.threads.write().await;
        if !threads.contains_key(thread_id) {
            threads.insert(
                thread_id.to_string(),
                ThreadContext {
                    id: thread_id.to_string(),
                    active_subagent: None,
                    queue: vec![],
                    created_at: chrono::Utc::now(),
                },
            );
        }
    }

    /// List snapshots of all known threads.
    pub async fn list_threads(&self) -> Vec<ThreadContextSummary> {
        let threads = self.threads.read().await;
        threads
            .values()
            .map(|t| ThreadContextSummary {
                id: t.id.clone(),
                active_subagent: t.active_subagent.clone(),
                queue_len: t.queue.len(),
                created_at: t.created_at,
            })
            .collect()
    }

    /// Get a snapshot of a thread context.
    pub async fn get_thread_context(&self, thread_id: &str) -> Option<ThreadContextSummary> {
        let threads = self.threads.read().await;
        threads.get(thread_id).map(|t| ThreadContextSummary {
            id: t.id.clone(),
            active_subagent: t.active_subagent.clone(),
            queue_len: t.queue.len(),
            created_at: t.created_at,
        })
    }

    /// Switch the active subagent on a thread.
    ///
    /// This performs a thread context switch: the given subagent becomes the
    /// active context on the thread. Passing `None` clears the active context.
    pub async fn switch_thread_active_subagent(
        &self,
        thread_id: &str,
        subagent_id: Option<&str>,
    ) -> crate::Result<()> {
        if let Some(id) = subagent_id {
            let subagents = self.subagents.read().await;
            let handle = subagents.get(id).ok_or_else(|| crate::error::SyscityError::NotFound {
                resource: format!("Subagent '{}'", id),
            })?;
            if handle.thread_id != thread_id {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Subagent {} is bound to thread {}, not {}",
                    id, handle.thread_id, thread_id
                )));
            }
            if handle.status == SubagentStatus::Terminated || handle.status == SubagentStatus::Crashed {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Cannot switch to {:?} subagent {}",
                    handle.status, id
                )));
            }
        }

        self.ensure_thread(thread_id).await;

        let mut threads = self.threads.write().await;
        let thread = threads.get_mut(thread_id).ok_or_else(|| {
            crate::error::SyscityError::Internal(format!("Thread {} disappeared", thread_id))
        })?;
        thread.active_subagent = subagent_id.map(|s| s.to_string());
        info!(
            "Switched active subagent on thread {} to {:?}",
            thread_id, subagent_id
        );
        self.emit(crate::gateway::GatewayEvent::AcpThreadSwitched {
            thread_id: thread_id.to_string(),
            active_subagent: subagent_id.map(|s| s.to_string()),
        })
        .await;
        Ok(())
    }

    /// Migrate a subagent to a different thread.
    ///
    /// The subagent's `thread_id` is updated, the old thread clears its active
    /// subagent reference if it pointed to this subagent, and any queued thread
    /// messages addressed to this subagent are moved to the target thread.
    pub async fn migrate_subagent_thread(
        &self,
        subagent_id: &str,
        target_thread_id: &str,
    ) -> crate::Result<()> {
        let (old_thread_id, _status) = {
            let subagents = self.subagents.read().await;
            let handle = subagents.get(subagent_id).ok_or_else(|| {
                crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                }
            })?;
            if handle.status == SubagentStatus::Terminated || handle.status == SubagentStatus::Crashed {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Cannot migrate {:?} subagent {}",
                    handle.status, subagent_id
                )));
            }
            (handle.thread_id.clone(), handle.status)
        };

        if old_thread_id == target_thread_id {
            return Ok(());
        }

        self.ensure_thread(target_thread_id).await;

        let moved_messages = {
            let mut threads = self.threads.write().await;
            let mut taken = Vec::new();
            if let Some(old) = threads.get_mut(&old_thread_id) {
                if old.active_subagent.as_deref() == Some(subagent_id) {
                    old.active_subagent = None;
                }
                let (for_subagent, remaining): (Vec<ThreadMessage>, Vec<ThreadMessage>) = old
                    .queue
                    .drain(..)
                    .partition(|m| m.subagent_id == subagent_id);
                old.queue = remaining;
                taken = for_subagent;
            }
            let target = threads.get_mut(target_thread_id).ok_or_else(|| {
                crate::error::SyscityError::Internal(format!(
                    "Thread {} disappeared after creation",
                    target_thread_id
                ))
            })?;
            if matches!(
                target.active_subagent.as_deref(),
                Some(id) if id != subagent_id
            ) {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Thread {} already has active subagent {}",
                    target_thread_id,
                    target.active_subagent.as_deref().unwrap_or("")
                )));
            }
            target.active_subagent = Some(subagent_id.to_string());
            let moved = taken.len();
            target.queue.extend(taken);
            moved
        };

        {
            let mut subagents = self.subagents.write().await;
            if let Some(handle) = subagents.get_mut(subagent_id) {
                handle.thread_id = target_thread_id.to_string();
            }
        }

        info!(
            "Migrated subagent {} from thread {} to thread {} (moved {} queued messages)",
            subagent_id, old_thread_id, target_thread_id, moved_messages
        );
        Ok(())
    }

    // ------------------------------------------------------------------
    // Cross-session subagent bus
    // ------------------------------------------------------------------

    /// Subscribe a subagent to a bus topic.
    pub async fn bus_subscribe(&self, subagent_id: &str, topic: &str) -> crate::Result<()> {
        {
            let subagents = self.subagents.read().await;
            if !subagents.contains_key(subagent_id) {
                return Err(crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                });
            }
        }
        let mut bus = self.bus.write().await;
        bus.subscribe(subagent_id, topic);
        info!("Subagent {} subscribed to bus topic {}", subagent_id, topic);
        Ok(())
    }

    /// Unsubscribe a subagent from a bus topic.
    pub async fn bus_unsubscribe(&self, subagent_id: &str, topic: &str) {
        let mut bus = self.bus.write().await;
        bus.unsubscribe(subagent_id, topic);
        info!("Subagent {} unsubscribed from bus topic {}", subagent_id, topic);
    }

    /// Publish a message to a bus topic from a subagent.
    pub async fn bus_publish(
        &self,
        subagent_id: &str,
        topic: &str,
        payload: &str,
    ) -> crate::Result<BusMessage> {
        {
            let subagents = self.subagents.read().await;
            if !subagents.contains_key(subagent_id) {
                return Err(crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                });
            }
        }
        let mut bus = self.bus.write().await;
        let message = bus.publish(topic, subagent_id, payload);
        info!("Subagent {} published to bus topic {}", subagent_id, topic);
        Ok(message)
    }

    /// Poll pending bus messages for a subagent on a topic.
    pub async fn bus_poll(
        &self,
        subagent_id: &str,
        topic: &str,
    ) -> crate::Result<Vec<BusMessage>> {
        {
            let subagents = self.subagents.read().await;
            if !subagents.contains_key(subagent_id) {
                return Err(crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                });
            }
        }
        let mut bus = self.bus.write().await;
        Ok(bus.poll(subagent_id, topic))
    }

    /// Poll pending bus messages for a subagent across all subscribed topics.
    pub async fn bus_poll_all(
        &self,
        subagent_id: &str,
    ) -> crate::Result<HashMap<String, Vec<BusMessage>>> {
        {
            let subagents = self.subagents.read().await;
            if !subagents.contains_key(subagent_id) {
                return Err(crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                });
            }
        }
        let mut bus = self.bus.write().await;
        Ok(bus.poll_all(subagent_id))
    }

    /// List all bus topics.
    pub async fn bus_topics(&self) -> Vec<String> {
        let bus = self.bus.read().await;
        bus.topics()
    }

    /// List subscribers for a bus topic.
    pub async fn bus_subscribers(&self, topic: &str) -> Vec<String> {
        let bus = self.bus.read().await;
        bus.subscribers(topic)
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
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                })?;

        let (response_tx, response_rx) = oneshot::channel();

        subagent
            .command_tx
            .send(SubagentCommand::ProcessMessage { message, response_tx })
            .await
            .map_err(|_| {
                crate::error::SyscityError::Internal("Subagent command channel closed".to_string())
            })?;

        let result = response_rx.await.map_err(|_| {
            crate::error::SyscityError::Internal("Subagent response channel closed".to_string())
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
            drop(subagents);
            if let Some(ref store) = self.store {
                let _ = store
                    .update_subagent_run_status(subagent_id, "shutting_down")
                    .await;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

 /// Kill a subagent immediately (force abort)
    pub async fn kill_subagent(&self, subagent_id: &str) -> crate::Result<bool> {
        let mut subagents = self.subagents.write().await;

        if let Some(subagent) = subagents.get_mut(subagent_id) {
            subagent.status = SubagentStatus::Terminated;
            let _ = subagent.command_tx.send(SubagentCommand::Shutdown).await;
            subagent.abort_handle.abort();
            info!("Killed subagent {} (force abort)", subagent_id);
            drop(subagents);
            if let Some(ref store) = self.store {
                let _ = store.kill_subagent_run(subagent_id, "user").await;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

 /// Steer a subagent — cancel current execution and send a new message
    pub async fn steer_subagent(
        &self,
        subagent_id: &str,
        message: String,
    ) -> crate::Result<String> {
        let subagents = self.subagents.read().await;
        let subagent =
            subagents
                .get(subagent_id)
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                })?;

 // 1. Cancel any in-progress execution
        let _ = subagent.command_tx.send(SubagentCommand::Cancel).await;

 // 2. Build steer message
        let steer_msg = IncomingMessage::new(
            "user".to_string(),
            format!("steer-{}", subagent_id),
            message.clone(),
        );

 // 3. Send steer message as new ProcessMessage
        let (response_tx, response_rx) = oneshot::channel();
        subagent
            .command_tx
            .send(SubagentCommand::ProcessMessage {
                message: steer_msg,
                response_tx,
            })
            .await
            .map_err(|_| {
                crate::error::SyscityError::Internal("Subagent command channel closed".to_string())
            })?;

        drop(subagents);

 // Persist steer event
        if let Some(ref store) = self.store {
            let _ = store.append_steer_to_run(subagent_id, &message).await;
        }

        match response_rx.await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                Err(crate::error::SyscityError::Internal("Steer response dropped".to_string()))
            }
        }
    }

 /// Terminate all subagents in a session
    pub async fn terminate_session(&self, session_id: &AcpSessionId) -> crate::Result<usize> {
        let sessions = self.sessions.read().await;
        let session =
            sessions
                .get(session_id)
                .ok_or_else(|| crate::error::SyscityError::NotFound {
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
        drop(sessions);

 // Delete from persistent store
        if let Some(ref store) = self.store {
            let _ = store.delete_acp_session(&session_id.0).await;
        }

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
        Self::new(50)
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

    /// Spawn a subagent whose task panics and verify it is automatically recovered.
    #[tokio::test]
    async fn test_subagent_crash_auto_recovery() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static CRASHED: AtomicBool = AtomicBool::new(false);
        let acp = AcpControlPlane::new(50)
            .with_recovery(CrashRecoveryConfig {
                enabled: true,
                max_retries: 1,
                backoff_seconds: &[0],
            })
            .with_agent_builder(|| {
                let provider = Arc::new(crate::providers::mock::MockProvider::new().with_callback(
                    |_messages| {
                        if !CRASHED.swap(true, Ordering::SeqCst) {
                            panic!("simulated subagent crash")
                        }
                        crate::providers::Message::assistant("recovered")
                    },
                ));
                let tools = Arc::new(crate::tools::ToolRegistry::new());
                let config = AgentConfig::default();
                Ok(Agent::new(config, provider, tools))
            });

        let session_id = acp.create_session("parent".to_string()).await;
        let config = SubagentConfig {
            retry_on_crash: true,
            max_crash_retries: 1,
            mode: SpawnMode::Run,
            ..SubagentConfig::default()
        };

        let handle = acp
            .spawn_subagent(session_id.clone(), "parent".to_string(), config)
            .await
            .expect("spawn subagent");

        // Send a message to trigger processing (and the simulated panic).
        let msg = IncomingMessage::new(
            "user".to_string(),
            format!("conv-{}", handle.id),
            "trigger".to_string(),
        );
        let _ = acp.send_message(&handle.id, msg).await;

        // Wait long enough for the panic + recovery to complete.
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;

        // The original handle is replaced during recovery; it may no longer be
        // present in the subagent map.
        let original_status = acp.get_subagent_status(&handle.id).await;
        assert!(
            original_status.is_none() || original_status == Some(SubagentStatus::Crashed),
            "original handle should be removed or marked Crashed"
        );

        // Session should contain a recovered subagent with a different id.
        let session_subagents = acp.list_session_subagents(&session_id).await;
        assert_eq!(session_subagents.len(), 1);
        let recovered = &session_subagents[0];
        assert_ne!(recovered.id, handle.id);
        assert_eq!(recovered.crash_count, 1);
        assert_eq!(recovered.status, SubagentStatus::Ready);

        // Cleanup
        let _ = acp.shutdown_subagent(&recovered.id).await;
    }

    #[tokio::test]
    async fn test_thread_context_switch_and_migration() {
        let acp = AcpControlPlane::new(50).with_agent_builder(mock_agent_builder());
        let session_id = acp.create_session("parent".to_string()).await;

        let s1 = acp
            .spawn_subagent(
                session_id.clone(),
                "parent".to_string(),
                SubagentConfig {
                    thread_binding: ThreadBinding::Thread("thread-a".to_string()),
                    ..SubagentConfig::default()
                },
            )
            .await
            .expect("spawn s1");

        let s2 = acp
            .spawn_subagent(
                session_id.clone(),
                "parent".to_string(),
                SubagentConfig {
                    thread_binding: ThreadBinding::Thread("thread-a".to_string()),
                    ..SubagentConfig::default()
                },
            )
            .await
            .expect("spawn s2");

        // Context switch: make s1 the active subagent on thread-a.
        acp.switch_thread_active_subagent("thread-a", Some(&s1.id))
            .await
            .expect("switch to s1");
        let ctx_a = acp.get_thread_context("thread-a").await.expect("thread-a exists");
        assert_eq!(ctx_a.active_subagent, Some(s1.id.clone()));

        // Migrate s1 to thread-b.
        acp.migrate_subagent_thread(&s1.id, "thread-b")
            .await
            .expect("migrate to thread-b");

        // s1 should now be bound to thread-b.
        let session_subagents = acp.list_session_subagents(&session_id).await;
        let s1_after = session_subagents
            .iter()
            .find(|h| h.id == s1.id)
            .expect("s1 still registered");
        assert_eq!(s1_after.thread_id, "thread-b");

        // thread-a should have cleared its active subagent.
        let ctx_a = acp.get_thread_context("thread-a").await.expect("thread-a exists");
        assert!(ctx_a.active_subagent.is_none());

        // thread-b should have s1 as active subagent.
        let ctx_b = acp.get_thread_context("thread-b").await.expect("thread-b exists");
        assert_eq!(ctx_b.active_subagent, Some(s1.id.clone()));

        // s2 should remain on thread-a.
        let s2_after = session_subagents
            .iter()
            .find(|h| h.id == s2.id)
            .expect("s2 still registered");
        assert_eq!(s2_after.thread_id, "thread-a");

        // Context switch s2 to active on thread-a.
        acp.switch_thread_active_subagent("thread-a", Some(&s2.id))
            .await
            .expect("switch to s2");
        let ctx_a = acp.get_thread_context("thread-a").await.expect("thread-a exists");
        assert_eq!(ctx_a.active_subagent, Some(s2.id.clone()));

        // Cleanup
        let _ = acp.shutdown_subagent(&s1.id).await;
        let _ = acp.shutdown_subagent(&s2.id).await;
    }

    #[tokio::test]
    async fn test_cross_session_subagent_bus() {
        let acp = AcpControlPlane::new(50).with_agent_builder(mock_agent_builder());
        let session_a = acp.create_session("parent-a".to_string()).await;
        let session_b = acp.create_session("parent-b".to_string()).await;

        let s1 = acp
            .spawn_subagent(session_a, "parent-a".to_string(), SubagentConfig::default())
            .await
            .expect("spawn s1");
        let s2 = acp
            .spawn_subagent(session_b, "parent-b".to_string(), SubagentConfig::default())
            .await
            .expect("spawn s2");

        // Subscribe s2 to the shared topic; s1 will publish without subscribing.
        acp.bus_subscribe(&s2.id, "alerts").await.expect("subscribe s2");

        // Publish from s1 in session A.
        let msg = acp
            .bus_publish(&s1.id, "alerts", "hello from session A")
            .await
            .expect("publish");
        assert_eq!(msg.sender_id, s1.id);
        assert_eq!(msg.payload, "hello from session A");

        // s2 in session B receives the message.
        let pending = acp.bus_poll(&s2.id, "alerts").await.expect("poll s2");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].payload, "hello from session A");

        // A second poll returns nothing new.
        let pending_again = acp.bus_poll(&s2.id, "alerts").await.expect("poll s2 again");
        assert!(pending_again.is_empty());

        // Topic and subscriber introspection.
        let topics = acp.bus_topics().await;
        assert!(topics.contains(&"alerts".to_string()));

        let subscribers = acp.bus_subscribers("alerts").await;
        assert_eq!(subscribers, vec![s2.id.clone()]);

        // Unsubscribe s2 and confirm it no longer receives messages.
        acp.bus_unsubscribe(&s2.id, "alerts").await;
        acp.bus_publish(&s1.id, "alerts", "after unsubscribe")
            .await
            .expect("publish after unsubscribe");
        let after_unsub = acp.bus_poll(&s2.id, "alerts").await.expect("poll after unsub");
        assert!(after_unsub.is_empty());

        // Cleanup
        let _ = acp.shutdown_subagent(&s1.id).await;
        let _ = acp.shutdown_subagent(&s2.id).await;
    }

    fn mock_agent_builder() -> impl Fn() -> crate::Result<Agent> + Send + Sync + 'static {
        || {
            let provider = Arc::new(crate::providers::mock::MockProvider::new().with_responses(vec![
                crate::providers::Message::assistant("mock response"),
            ]));
            let tools = Arc::new(crate::tools::ToolRegistry::new());
            let config = AgentConfig::default();
            Ok(Agent::new(config, provider, tools))
        }
    }

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
        let acp = AcpControlPlane::new(50);
        let subagents = acp.list_subagents().await;
        assert!(subagents.is_empty());
    }

    #[tokio::test]
    async fn test_create_session() {
        let acp = AcpControlPlane::new(50);
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
        let acp = AcpControlPlane::new(50);
        let info = acp
            .get_session_info(&AcpSessionId("nonexistent".to_string()))
            .await;
        assert!(info.is_none());
    }

    #[tokio::test]
    async fn test_terminate_session_not_found() {
        let acp = AcpControlPlane::new(50);
        let result = acp
            .terminate_session(&AcpSessionId("nonexistent".to_string()))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_session_subagents_empty() {
        let acp = AcpControlPlane::new(50);
        let session_id = acp.create_session("parent".to_string()).await;
        let subagents = acp.list_session_subagents(&session_id).await;
        assert!(subagents.is_empty());
    }

    #[tokio::test]
    async fn test_get_subagent_status_not_found() {
        let acp = AcpControlPlane::new(50);
        let status = acp.get_subagent_status("nonexistent").await;
        assert!(status.is_none());
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

    #[tokio::test]
    async fn test_concurrent_subagent_spawn() {
        let acp = AcpControlPlane::new(50);
        acp.set_agent_builder(mock_agent_builder()).await;
        let session_id = acp.create_session("parent-1".to_string()).await;

        let mut spawn_tasks = Vec::new();
        for i in 0..10usize {
            let acp_clone = acp.clone();
            let sid = session_id.clone();
            let config = SubagentConfig {
                agent_type: "default".to_string(),
                mode: SpawnMode::Run,
                thread_binding: ThreadBinding::Auto,
                system_prompt: Some(format!("subagent-{}", i)),
                max_tokens: None,
                temperature: None,
                tools: vec![],
                context: None,
                timeout_seconds: Some(30),
                retry_on_crash: false,
                max_crash_retries: 0,
            };
            spawn_tasks.push(tokio::spawn(async move {
                acp_clone.spawn_subagent(sid, "parent-1".to_string(), config).await
            }));
        }

        let results = futures::future::join_all(spawn_tasks).await;
        let mut handles = Vec::new();
        for result in results {
            let handle = result
                .expect("spawn task should not panic")
                .expect("spawn_subagent should succeed");
            assert!(
                handle.command_tx.send(SubagentCommand::Shutdown).await.is_ok(),
                "subagent should accept shutdown"
            );
            handles.push(handle);
        }

 // All 10 subagents should have been created with unique IDs
        assert_eq!(handles.len(), 10);
        let ids: std::collections::HashSet<_> = handles.iter().map(|h| h.id.clone()).collect();
        assert_eq!(ids.len(), 10, "all subagent IDs should be unique");
    }

    #[tokio::test]
    async fn test_acp_lifecycle_events_are_emitted() {
        let acp = AcpControlPlane::new(50).with_agent_builder(mock_agent_builder());
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(16);
        acp.set_event_tx(event_tx).await;

        let session_id = acp.create_session("parent".to_string()).await;
        let handle = acp
            .spawn_subagent(
                session_id.clone(),
                "parent".to_string(),
                SubagentConfig {
                    agent_type: "default".to_string(),
                    mode: SpawnMode::Run,
                    thread_binding: ThreadBinding::New,
                    ..SubagentConfig::default()
                },
            )
            .await
            .expect("spawn subagent");

        let event = event_rx.recv().await.expect("receive spawned event");
        match event {
            crate::gateway::GatewayEvent::AcpSpawned {
                session_id: sid,
                subagent_id,
                parent_id,
                mode,
                ..
            } => {
                assert_eq!(sid, session_id.to_string());
                assert_eq!(subagent_id, handle.id);
                assert_eq!(parent_id, "parent");
                assert_eq!(mode, "run");
            }
            other => panic!("expected AcpSpawned event, got {:?}", other),
        }

        let _ = handle.command_tx.send(SubagentCommand::Shutdown).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let completed = event_rx
            .recv()
            .await
            .expect("receive completed event");
        match completed {
            crate::gateway::GatewayEvent::AcpCompleted {
                subagent_id,
                status,
                ..
            } => {
                assert_eq!(subagent_id, handle.id);
                assert_eq!(status, "terminated");
            }
            other => panic!("expected AcpCompleted event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_acp_control_plane_has_store_without_store() {
        let acp = AcpControlPlane::new(50);
        assert!(!acp.has_store());
    }
}
