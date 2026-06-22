use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::agent::session_store::SaveSubagentRunParams;
use crate::agent::{Agent, ProgressCallback};
use crate::channels::IncomingMessage;

use super::bus::{AcpBus, BusMessage};
use super::config::{
    AcpSessionId, AcpSessionStatus, CrashRecoveryConfig, SpawnMode, SubagentConfig, SubagentStatus,
    ThreadBinding, ThreadContext, ThreadContextSummary,
};
use super::controller::ExecutionController;
use super::session::{acp_actor_loop, AcpCommand, ActorContext};
use super::subagent::{SubagentCommand, SubagentHandle};

/// ACP Control Plane - unified control plane for agents and subagents
#[derive(Clone)]
pub struct AcpControlPlane {
    /// Subagents by ID
    pub(super) subagents: Arc<RwLock<HashMap<String, SubagentHandle>>>,
    /// Threads by ID
    pub(super) threads: Arc<RwLock<HashMap<String, ThreadContext>>>,
    /// ACP sessions
    pub(super) sessions: Arc<RwLock<HashMap<AcpSessionId, AcpSession>>>,
    /// Default agent builder (set after initialization when provider/tools are
    /// ready)
    #[allow(clippy::type_complexity)]
    pub(super) default_agent_builder:
        Arc<RwLock<Option<Arc<dyn Fn() -> crate::Result<Agent> + Send + Sync>>>>,
    /// Command channel to the ACP actor loop
    pub(super) command_tx: mpsc::Sender<AcpCommand>,
    /// Optional session store for persisting subagent run records
    pub(super) store: Option<Arc<crate::agent::session_store::SessionStore>>,
    /// Maximum iterations per ACP execution
    pub(super) max_iterations: usize,
    /// Configuration controlling automatic crash recovery.
    pub(super) recovery: Arc<RwLock<CrashRecoveryConfig>>,
    /// Cross-session subagent communication bus.
    pub(super) bus: Arc<RwLock<AcpBus>>,
    /// Event broadcast channel for ACP lifecycle events.
    pub(super) event_tx:
        Arc<RwLock<Option<tokio::sync::broadcast::Sender<crate::gateway::GatewayEvent>>>>,
    /// Handle to the ACP actor task for graceful shutdown.
    pub(super) actor_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
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
        let acp = Self {
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
            actor_handle: Arc::new(Mutex::new(None)),
        };
        let handle = tokio::spawn(acp_actor_loop(
            command_rx,
            ActorContext {
                max_iterations,
                default_agent_builder: Arc::clone(&acp.default_agent_builder),
                control_plane: acp.clone(),
            },
        ));
        *acp.actor_handle
            .try_lock()
            .expect("actor handle lock available during construction") = Some(handle);
        acp
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
            let mut guard = self
                .default_agent_builder
                .try_write()
                .expect("agent builder lock available during construction");
            *guard = Some(Arc::new(builder));
        }
        self
    }

    /// Configure automatic crash recovery.
    pub fn with_recovery(self, recovery: CrashRecoveryConfig) -> Self {
        {
            let mut guard = self
                .recovery
                .try_write()
                .expect("recovery lock available during construction");
            *guard = recovery;
        }
        self
    }

    /// Update crash recovery configuration at runtime.
    pub async fn set_recovery_config(&self, recovery: CrashRecoveryConfig) {
        info!("ACP crash recovery config updated: enabled={}", recovery.enabled);
        let mut guard = self.recovery.write().await;
        *guard = recovery;
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
    pub(crate) async fn emit(&self, event: crate::gateway::GatewayEvent) {
        let guard = self.event_tx.read().await;
        if let Some(ref tx) = *guard {
            if let Err(e) = tx.send(event) {
                warn!("Failed to emit ACP event: {}", e);
            }
        }
    }

    /// Returns a clone of the ACP command channel sender.
    pub fn command_tx(&self) -> mpsc::Sender<AcpCommand> {
        self.command_tx.clone()
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
                            "Skipping malformed persisted ACP session (session_id={}, \
                             parent_id={})",
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
    ) -> crate::Result<crate::channels::OutgoingMessage> {
        self.execute_session_with_max_iterations(agent, message, None)
            .await
    }

    /// Execute a message in persistent session mode with optional max iteration
    /// override.
    pub async fn execute_session_with_max_iterations(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
        max_iterations: Option<usize>,
    ) -> crate::Result<crate::channels::OutgoingMessage> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AcpCommand::ExecuteSession {
                agent,
                message,
                max_iterations,
                respond_to: tx,
            })
            .await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?
    }

    /// Execute a message in one-shot run mode
    pub async fn execute_run(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
    ) -> crate::Result<crate::channels::OutgoingMessage> {
        self.execute_run_with_max_iterations(agent, message, None)
            .await
    }

    /// Execute a message in one-shot run mode with optional max iteration
    /// override.
    pub async fn execute_run_with_max_iterations(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
        max_iterations: Option<usize>,
    ) -> crate::Result<crate::channels::OutgoingMessage> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AcpCommand::ExecuteRun {
                agent,
                message,
                max_iterations,
                respond_to: tx,
            })
            .await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?
    }

    /// Execute with progress callbacks in session mode
    pub async fn execute_session_with_progress(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
    ) -> crate::Result<crate::channels::OutgoingMessage> {
        self.execute_session_with_progress_and_max_iterations(agent, message, progress_cb, None)
            .await
    }

    /// Execute with progress callbacks in session mode with optional max
    /// iteration override.
    pub async fn execute_session_with_progress_and_max_iterations(
        &self,
        agent: Arc<Agent>,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
        max_iterations: Option<usize>,
    ) -> crate::Result<crate::channels::OutgoingMessage> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AcpCommand::ExecuteSessionWithProgress {
                agent,
                message,
                progress_cb,
                max_iterations,
                respond_to: tx,
            })
            .await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?
    }

    /// Pause a running session
    pub async fn pause(&self, session_id: String) -> crate::Result<()> {
        self.command_tx
            .send(AcpCommand::Pause { session_id })
            .await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?;
        Ok(())
    }

    /// Resume a paused session
    pub async fn resume(&self, session_id: String) -> crate::Result<()> {
        self.command_tx
            .send(AcpCommand::Resume { session_id })
            .await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?;
        Ok(())
    }

    /// Single step a paused session
    pub async fn step(&self, session_id: String) -> crate::Result<()> {
        self.command_tx
            .send(AcpCommand::Step { session_id })
            .await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?;
        Ok(())
    }

    /// Cancel a running session
    pub async fn cancel(&self, session_id: String) -> crate::Result<()> {
        self.command_tx
            .send(AcpCommand::Cancel { session_id })
            .await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?;
        Ok(())
    }

    /// Get session status
    pub async fn get_status(&self, session_id: String) -> crate::Result<Option<AcpSessionStatus>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.command_tx
            .send(AcpCommand::GetStatus { session_id, respond_to: tx })
            .await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?;
        rx.await.map_err(|_| {
            crate::error::SyscityError::Internal("ACP response channel closed".to_string())
        })
    }

    /// Shutdown the ACP actor loop
    pub async fn shutdown(&self) -> crate::Result<()> {
        self.command_tx
            .send(AcpCommand::Shutdown)
            .await
            .map_err(|_| crate::error::SyscityError::Internal("ACP channel closed".to_string()))?;

        let handle = {
            let mut guard = self.actor_handle.lock().await;
            guard.take()
        };
        if let Some(handle) = handle {
            match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(crate::error::SyscityError::Internal(format!(
                    "ACP actor task panicked: {}",
                    e
                ))),
                Err(_) => Err(crate::error::SyscityError::Internal(
                    "ACP actor shutdown timed out".to_string(),
                )),
            }
        } else {
            Ok(())
        }
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
            if let Err(e) = store
                .save_acp_session(&session_id.0, &parent_agent_id, &[], chrono::Utc::now())
                .await
            {
                warn!("Failed to persist new ACP session {}: {}", session_id.0, e);
            }
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
                                        data: _,
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
                        if let Err(e) = store
                            .complete_subagent_run(&watch_id, None, Some("panicked"))
                            .await
                        {
                            warn!("Failed to persist crash completion for {}: {}", watch_id, e);
                        }
                    }
                    // Log crash for external recovery (call recover_crashed_subagent to restart)
                    let recovery_enabled = {
                        let global = acp_for_recovery.recovery.read().await;
                        global.enabled && current_crash_count < global.max_retries
                    };
                    if recovery_enabled {
                        warn!(
                            "Subagent {} crashed (attempt {}/{}). Auto-recovery enabled — call \
                             acp.recover_crashed_subagent() to restart.",
                            watch_id,
                            current_crash_count + 1,
                            {
                                let global = acp_for_recovery.recovery.read().await;
                                global.max_retries
                            }
                        );
                    }

                    // Automatic recovery: if enabled globally, schedule a recovery command to the
                    // ACP actor loop so that recovery does not create a direct async recursion
                    // cycle with `spawn_subagent`.
                    let acp = acp_for_recovery.clone();
                    let sid = recovery_session_id.clone();
                    let pid = recovery_parent_id.clone();
                    let cfg = recovery_config_clone.clone();
                    tokio::spawn(async move {
                        let (should_recover, delay) = {
                            let global = acp.recovery.read().await;
                            let should_recover =
                                global.enabled && current_crash_count < global.max_retries;
                            let delay = if should_recover {
                                global
                                    .backoff_seconds
                                    .get(current_crash_count as usize)
                                    .copied()
                                    .unwrap_or_else(|| {
                                        global.backoff_seconds.last().copied().unwrap_or(30)
                                    })
                            } else {
                                0
                            };
                            (should_recover, delay)
                        };

                        if !should_recover {
                            return;
                        }

                        warn!(
                            "Auto-recovering subagent {} (attempt {}/{}) in {}s",
                            watch_id,
                            current_crash_count + 1,
                            {
                                let global = acp.recovery.read().await;
                                global.max_retries
                            },
                            delay
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(delay)).await;
                        let cmd = AcpCommand::RecoverSubagent {
                            session_id: sid,
                            parent_id: pid,
                            config: cfg,
                            crash_count: current_crash_count + 1,
                        };
                        if let Err(e) = acp.command_tx.send(cmd).await {
                            warn!("Failed to schedule recovery command for {}: {}", watch_id, e);
                        }
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
        session_id: AcpSessionId,
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
            let handle = subagents
                .get(id)
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", id),
                })?;
            if handle.thread_id != thread_id {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Subagent {} is bound to thread {}, not {}",
                    id, handle.thread_id, thread_id
                )));
            }
            if handle.status == SubagentStatus::Terminated
                || handle.status == SubagentStatus::Crashed
            {
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
        info!("Switched active subagent on thread {} to {:?}", thread_id, subagent_id);
        self.emit(crate::gateway::GatewayEvent::AcpThreadSwitched {
            thread_id: thread_id.to_string(),
            active_subagent: subagent_id.map(|s| s.to_string()),
        })
        .await;
        Ok(())
    }

    /// Migrate a subagent to a different thread.
    ///
    /// The subagent's `thread_id` is updated and the old thread clears its
    /// active subagent reference if it pointed to this subagent.
    pub async fn migrate_subagent_thread(
        &self,
        subagent_id: &str,
        target_thread_id: &str,
    ) -> crate::Result<()> {
        let (old_thread_id, _status) = {
            let subagents = self.subagents.read().await;
            let handle =
                subagents
                    .get(subagent_id)
                    .ok_or_else(|| crate::error::SyscityError::NotFound {
                        resource: format!("Subagent '{}'", subagent_id),
                    })?;
            if handle.status == SubagentStatus::Terminated
                || handle.status == SubagentStatus::Crashed
            {
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

        {
            let mut threads = self.threads.write().await;
            if let Some(old) = threads.get_mut(&old_thread_id) {
                if old.active_subagent.as_deref() == Some(subagent_id) {
                    old.active_subagent = None;
                }
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
        }

        {
            let mut subagents = self.subagents.write().await;
            if let Some(handle) = subagents.get_mut(subagent_id) {
                handle.thread_id = target_thread_id.to_string();
            }
        }

        info!(
            "Migrated subagent {} from thread {} to thread {}",
            subagent_id, old_thread_id, target_thread_id
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
    pub async fn bus_poll(&self, subagent_id: &str, topic: &str) -> crate::Result<Vec<BusMessage>> {
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

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        subagent
            .command_tx
            .send(SubagentCommand::ProcessMessage {
                message: Box::new(message),
                response_tx,
            })
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
            if let Err(e) = subagent.command_tx.send(SubagentCommand::Shutdown).await {
                warn!("Failed to send shutdown command to subagent {}: {}", subagent_id, e);
            }
            // Watchdog task will update status to Terminated once the task exits.
            drop(subagents);
            if let Some(ref store) = self.store {
                if let Err(e) = store
                    .update_subagent_run_status(subagent_id, "shutting_down")
                    .await
                {
                    warn!("Failed to persist shutting_down status for {}: {}", subagent_id, e);
                }
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
            if let Err(e) = subagent.command_tx.send(SubagentCommand::Shutdown).await {
                warn!("Failed to send shutdown command to subagent {}: {}", subagent_id, e);
            }
            subagent.abort_handle.abort();
            subagent.status = SubagentStatus::Terminated;
            info!("Killed subagent {} (force abort)", subagent_id);
            drop(subagents);
            if let Some(ref store) = self.store {
                if let Err(e) = store.kill_subagent_run(subagent_id, "user").await {
                    warn!("Failed to persist kill event for {}: {}", subagent_id, e);
                }
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
        if let Err(e) = subagent.command_tx.send(SubagentCommand::Cancel).await {
            warn!("Failed to send cancel command to subagent {}: {}", subagent_id, e);
        }

        // 2. Build steer message
        let steer_msg = IncomingMessage::new(
            "user".to_string(),
            format!("steer-{}", subagent_id),
            message.clone(),
        );

        // 3. Send steer message as new ProcessMessage
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        subagent
            .command_tx
            .send(SubagentCommand::ProcessMessage {
                message: Box::new(steer_msg),
                response_tx,
            })
            .await
            .map_err(|_| {
                crate::error::SyscityError::Internal("Subagent command channel closed".to_string())
            })?;

        drop(subagents);

        // Persist steer event
        if let Some(ref store) = self.store {
            if let Err(e) = store.append_steer_to_run(subagent_id, &message).await {
                warn!("Failed to persist steer event for {}: {}", subagent_id, e);
            }
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
            if self.shutdown_subagent(&subagent_id).await.unwrap_or(false) {
                count += 1;
            }
        }

        // Remove session
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        drop(sessions);

        // Delete from persistent store
        if let Some(ref store) = self.store {
            if let Err(e) = store.delete_acp_session(&session_id.0).await {
                warn!("Failed to delete ACP session {} from store: {}", session_id.0, e);
            }
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
    pub async fn list_session_subagents(
        &self,
        session_id: &AcpSessionId,
    ) -> Vec<SubagentHandle> {
        let subagent_ids = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .map(|s| s.subagents.clone())
                .unwrap_or_default()
        };

        let subagents = self.subagents.read().await;
        subagent_ids
            .into_iter()
            .filter_map(|id| subagents.get(&id).cloned())
            .collect()
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
    pub async fn get_subagent_tree(
        &self,
        session_id: &AcpSessionId,
    ) -> Vec<SubagentTreeNode> {
        let (root_parent_id, session_subagent_ids) = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .map(|s| (s.parent_agent_id.clone(), s.subagents.clone()))
                .unwrap_or_default()
        };

        let subagents = self.subagents.read().await;
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

/// Session info for display
#[derive(Debug, Clone)]
pub struct AcpSessionInfo {
    pub id: AcpSessionId,
    pub parent_agent_id: String,
    pub subagent_count: usize,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Subagent tree node for hierarchical display
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubagentTreeNode {
    pub id: String,
    pub parent_id: String,
    pub status: SubagentStatus,
    pub mode: SpawnMode,
    pub thread_id: String,
    pub children: Vec<SubagentTreeNode>,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::agent::{Agent, AgentConfig};

    fn mock_agent_builder() -> impl Fn() -> crate::Result<Agent> + Send + Sync + 'static {
        || {
            let provider = Arc::new(
                crate::providers::mock::MockProvider::new()
                    .with_responses(vec![crate::providers::Message::assistant("mock response")]),
            );
            let tools = Arc::new(crate::tools::ToolRegistry::new());
            let config = AgentConfig::default();
            Ok(Agent::new(config, provider, tools))
        }
    }

    /// Spawn a subagent whose task panics and verify it is automatically
    /// recovered.
    #[tokio::test]
    async fn test_subagent_crash_auto_recovery() {
        use std::sync::atomic::{AtomicBool, Ordering};

        static CRASHED: AtomicBool = AtomicBool::new(false);
        let acp = AcpControlPlane::new(50)
            .with_recovery(CrashRecoveryConfig {
                enabled: true,
                max_retries: 1,
                backoff_seconds: vec![0],
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
        let _ = acp.send_message(&handle.id, msg).await.ok();

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
        acp.shutdown_subagent(&recovered.id)
            .await
            .expect("shutdown recovered subagent");
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
        let ctx_a = acp
            .get_thread_context("thread-a")
            .await
            .expect("thread-a exists");
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
        let ctx_a = acp
            .get_thread_context("thread-a")
            .await
            .expect("thread-a exists");
        assert!(ctx_a.active_subagent.is_none());

        // thread-b should have s1 as active subagent.
        let ctx_b = acp
            .get_thread_context("thread-b")
            .await
            .expect("thread-b exists");
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
        let ctx_a = acp
            .get_thread_context("thread-a")
            .await
            .expect("thread-a exists");
        assert_eq!(ctx_a.active_subagent, Some(s2.id.clone()));

        // Cleanup
        acp.shutdown_subagent(&s1.id)
            .await
            .expect("shutdown s1");
        acp.shutdown_subagent(&s2.id)
            .await
            .expect("shutdown s2");
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
        acp.bus_subscribe(&s2.id, "alerts")
            .await
            .expect("subscribe s2");

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
        let after_unsub = acp
            .bus_poll(&s2.id, "alerts")
            .await
            .expect("poll after unsub");
        assert!(after_unsub.is_empty());

        // Cleanup
        acp.shutdown_subagent(&s1.id)
            .await
            .expect("shutdown s1");
        acp.shutdown_subagent(&s2.id)
            .await
            .expect("shutdown s2");
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
    async fn test_concurrent_subagent_spawn() {
        let acp = AcpControlPlane::new(50);
        acp.set_agent_builder(mock_agent_builder()).await;
        let session_id = acp.create_session("parent-1".to_string()).await;

        let mut spawn_tasks = Vec::new();
        for i in 0..10usize {
            let acp_clone = acp.clone();
            let sid = session_id.clone();
            let config = SubagentConfig {
                mode: SpawnMode::Run,
                thread_binding: ThreadBinding::Auto,
                system_prompt: Some(format!("subagent-{}", i)),
                timeout_seconds: Some(30),
            };
            spawn_tasks.push(tokio::spawn(async move {
                acp_clone
                    .spawn_subagent(sid, "parent-1".to_string(), config)
                    .await
            }));
        }

        let results = futures::future::join_all(spawn_tasks).await;
        let mut handles = Vec::new();
        for result in results {
            let handle = result
                .expect("spawn task should not panic")
                .expect("spawn_subagent should succeed");
            assert!(
                handle
                    .command_tx
                    .send(SubagentCommand::Shutdown)
                    .await
                    .is_ok(),
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

        let _ = handle
            .command_tx
            .send(SubagentCommand::Shutdown)
            .await
            .expect("send shutdown to subagent");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let completed = event_rx.recv().await.expect("receive completed event");
        match completed {
            crate::gateway::GatewayEvent::AcpCompleted { subagent_id, status, .. } => {
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

    #[tokio::test]
    async fn test_pause_resume_step_cancel_emit_status_changed_after_actor_processing() {
        let acp = AcpControlPlane::new(50).with_agent_builder(mock_agent_builder());
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(16);
        acp.set_event_tx(event_tx).await;

        // Create a session actor by executing a message.
        let agent = mock_agent_builder()().expect("mock agent builds");
        let msg = IncomingMessage::new("user1", "conv1", "hello");
        let _ = acp.execute_session(Arc::new(agent), msg).await.ok();

        // Pause: event should report the actual state after the actor processed it.
        acp.pause("conv1".to_string())
            .await
            .expect("pause command sent");
        let event = event_rx.recv().await.expect("receive pause event");
        assert!(
            matches!(
                event,
                crate::gateway::GatewayEvent::AcpStatusChanged {
                    ref session_id,
                    ref runtime_state,
                } if session_id == "conv1" && runtime_state == "paused"
            ),
            "expected AcpStatusChanged(paused), got {:?}",
            event
        );

        // Resume.
        acp.resume("conv1".to_string())
            .await
            .expect("resume command sent");
        let event = event_rx.recv().await.expect("receive resume event");
        assert!(
            matches!(
                event,
                crate::gateway::GatewayEvent::AcpStatusChanged {
                    ref session_id,
                    ref runtime_state,
                } if session_id == "conv1" && runtime_state == "running"
            ),
            "expected AcpStatusChanged(running), got {:?}",
            event
        );

        // Step.
        acp.step("conv1".to_string())
            .await
            .expect("step command sent");
        let event = event_rx.recv().await.expect("receive step event");
        assert!(
            matches!(
                event,
                crate::gateway::GatewayEvent::AcpStatusChanged {
                    ref session_id,
                    ref runtime_state,
                } if session_id == "conv1" && runtime_state == "stepping"
            ),
            "expected AcpStatusChanged(stepping), got {:?}",
            event
        );

        // Cancel.
        acp.cancel("conv1".to_string())
            .await
            .expect("cancel command sent");
        let event = event_rx.recv().await.expect("receive cancel event");
        assert!(
            matches!(
                event,
                crate::gateway::GatewayEvent::AcpStatusChanged {
                    ref session_id,
                    ref runtime_state,
                } if session_id == "conv1" && runtime_state == "cancelled"
            ),
            "expected AcpStatusChanged(cancelled), got {:?}",
            event
        );
    }
}
