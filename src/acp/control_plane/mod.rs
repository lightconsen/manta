use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{info, warn};

use super::bus::AcpBus;
use super::config::{AcpSessionId, AcpSessionStatus, CrashRecoveryConfig};
use super::session::{acp_actor_loop, AcpCommand, ActorContext};
use crate::agent::{Agent, ProgressCallback};
use crate::channels::IncomingMessage;

mod bus_ops;
mod spawn;
mod subagent_ops;
mod thread;

#[cfg(test)]
mod tests;

/// ACP Control Plane - unified control plane for agents and subagents
#[derive(Clone)]
pub struct AcpControlPlane {
    /// Subagents by ID
    pub(crate) subagents: Arc<RwLock<HashMap<String, super::subagent::SubagentHandle>>>,
    /// Threads by ID
    pub(crate) threads: Arc<RwLock<HashMap<String, super::config::ThreadContext>>>,
    /// ACP sessions
    pub(crate) sessions: Arc<RwLock<HashMap<AcpSessionId, AcpSession>>>,
    /// Default agent builder (set after initialization when provider/tools are
    /// ready). Receives the id of the subagent being constructed so the agent
    /// can be tagged with it (observability, per-subagent configuration).
    #[allow(clippy::type_complexity)]
    pub(crate) default_agent_builder:
        Arc<RwLock<Option<Arc<dyn Fn(&str) -> crate::Result<Agent> + Send + Sync>>>>,
    /// Command channel to the ACP actor loop
    pub(crate) command_tx: mpsc::Sender<AcpCommand>,
    /// Optional session store for persisting subagent run records
    pub(crate) store: Option<Arc<crate::agent::session_store::SessionStore>>,
    /// Maximum iterations per ACP execution
    pub(crate) max_iterations: usize,
    /// Configuration controlling automatic crash recovery.
    pub(crate) recovery: Arc<RwLock<CrashRecoveryConfig>>,
    /// Cross-session subagent communication bus.
    pub(crate) bus: Arc<RwLock<AcpBus>>,
    /// Event broadcast channel for ACP lifecycle events.
    pub(crate) event_tx:
        Arc<RwLock<Option<tokio::sync::broadcast::Sender<crate::gateway::GatewayEvent>>>>,
    /// Handle to the ACP actor task for graceful shutdown.
    pub(crate) actor_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
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
        // The lock was just created above and no other task can hold it.
        #[allow(clippy::expect_used)]
        {
            *acp.actor_handle
                .try_lock()
                .expect("actor handle lock available during construction") = Some(handle);
        }
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
        F: Fn(&str) -> crate::Result<Agent> + Send + Sync + 'static,
    {
        {
            // The RwLock was created with `AcpControlPlane::new` and no other
            // task can hold it during this builder call.
            #[allow(clippy::expect_used)]
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
            // The RwLock was created with `AcpControlPlane::new` and no other
            // task can hold it during this builder call.
            #[allow(clippy::expect_used)]
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
        F: Fn(&str) -> crate::Result<Agent> + Send + Sync + 'static,
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
    // Session management
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
    pub status: super::config::SubagentStatus,
    pub mode: super::config::SpawnMode,
    pub thread_id: String,
    pub children: Vec<SubagentTreeNode>,
}
