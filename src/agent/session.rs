//! Multi-Agent Session Orchestration
//!
//! Inspired by OpenClaw's ACP session management, this provides:
//! - Multi-agent sessions with multiple agents collaborating
//! - Session thread binding (isolated, parent, shared, new)
//! - Agent lifecycle management within a session
//! - Context sharing between agents
//! - Intent-based agent routing

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::agent::personality::AgentPersonality;
use crate::channels::IncomingMessage;

/// Thread binding mode for agents in a session
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThreadBinding {
    /// New isolated thread - no shared context
    Isolated,
    /// Bind to parent's thread - inherits parent's context
    Parent,
    /// Bind to a specific existing thread
    Existing(String),
    /// Create new thread with shared memory
    Shared,
}

impl Default for ThreadBinding {
    fn default() -> Self {
        ThreadBinding::Isolated
    }
}

impl ThreadBinding {
    /// Get thread ID for this binding mode
    pub fn get_thread_id(&self, parent_thread: &str) -> String {
        match self {
            ThreadBinding::Isolated => format!("thread-{}", Uuid::new_v4()),
            ThreadBinding::Parent => parent_thread.to_string(),
            ThreadBinding::Existing(id) => id.clone(),
            ThreadBinding::Shared => format!("shared-{}", parent_thread),
        }
    }
}

/// Agent instance within a session
#[derive(Debug, Clone)]
pub struct SessionAgent {
    /// Agent ID
    pub id: String,
    /// Agent personality
    pub personality: AgentPersonality,
    /// Thread binding mode
    pub binding: ThreadBinding,
    /// Thread ID this agent is bound to
    pub thread_id: String,
    /// Whether agent is currently active
    pub is_active: bool,
    /// Agent status
    pub status: AgentInstanceStatus,
    /// Spawn time
    pub spawned_at: std::time::Instant,
    /// Last activity time
    pub last_activity: std::time::Instant,
}

/// Status of an agent instance
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentInstanceStatus {
    /// Agent is starting up
    Starting,
    /// Agent is ready to process messages
    Ready,
    /// Agent is busy processing
    Busy,
    /// Agent is shutting down
    ShuttingDown,
    /// Agent has terminated
    Terminated,
}

impl SessionAgent {
    /// Create a new session agent
    pub fn new(
        id: String,
        personality: AgentPersonality,
        binding: ThreadBinding,
        parent_thread: &str,
    ) -> Self {
        let thread_id = binding.get_thread_id(parent_thread);

        Self {
            id,
            personality,
            binding,
            thread_id,
            is_active: true,
            status: AgentInstanceStatus::Starting,
            spawned_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
        }
    }

    /// Mark agent as ready
    pub fn mark_ready(&mut self) {
        self.status = AgentInstanceStatus::Ready;
        self.last_activity = std::time::Instant::now();
    }

    /// Mark agent as busy
    pub fn mark_busy(&mut self) {
        self.status = AgentInstanceStatus::Busy;
        self.last_activity = std::time::Instant::now();
    }

    /// Mark agent as terminated
    pub fn mark_terminated(&mut self) {
        self.status = AgentInstanceStatus::Terminated;
        self.is_active = false;
    }

    /// Check if agent shares context with another agent
    pub fn shares_context_with(&self, other: &SessionAgent) -> bool {
        match (&self.binding, &other.binding) {
            // Both isolated - never share
            (ThreadBinding::Isolated, _) | (_, ThreadBinding::Isolated) => false,
            // Same thread ID - share context
            _ => self.thread_id == other.thread_id,
        }
    }
}

/// Multi-agent session for orchestrating multiple agents
#[derive(Debug)]
pub struct MultiAgentSession {
    /// Session ID
    pub id: String,
    /// Primary thread ID for this session
    pub primary_thread_id: String,
    /// Agents in this session
    agents: HashMap<String, SessionAgent>,
    /// Context shared across the session (for Shared binding mode)
    shared_context: Arc<RwLock<HashMap<String, String>>>,
    /// Session creation time
    pub created_at: std::time::Instant,
    /// Last activity time
    pub last_activity: std::time::Instant,
    /// Message channel for routing
    message_tx: mpsc::Sender<SessionMessage>,
}

/// Message within a session
#[derive(Debug)]
pub enum SessionMessage {
    /// Route message to specific agent
    RouteToAgent {
        agent_id: String,
        message: IncomingMessage,
    },
    /// Broadcast to all agents in session
    Broadcast {
        message: IncomingMessage,
        exclude_agent: Option<String>,
    },
    /// Spawn new agent in session
    SpawnAgent {
        agent_id: String,
        personality: AgentPersonality,
        binding: ThreadBinding,
    },
    /// Terminate agent
    TerminateAgent { agent_id: String },
    /// Get session status
    GetStatus {
        respond_to: oneshot::Sender<SessionStatus>,
    },
}

/// Session status
#[derive(Debug, Clone)]
pub struct SessionStatus {
    pub session_id: String,
    pub agent_count: usize,
    pub active_agents: Vec<String>,
    pub thread_count: usize,
}

impl MultiAgentSession {
    /// Create a new multi-agent session
    pub fn new(id: String) -> (Self, mpsc::Receiver<SessionMessage>) {
        let (message_tx, message_rx) = mpsc::channel(100);
        let primary_thread_id = format!("session-{}", id);

        let session = Self {
            id: id.clone(),
            primary_thread_id,
            agents: HashMap::new(),
            shared_context: Arc::new(RwLock::new(HashMap::new())),
            created_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
            message_tx,
        };

        (session, message_rx)
    }

    /// Get the message sender for this session
    pub fn sender(&self) -> mpsc::Sender<SessionMessage> {
        self.message_tx.clone()
    }

    /// Spawn an agent in this session
    pub fn spawn_agent(
        &mut self,
        agent_id: String,
        personality: AgentPersonality,
        binding: ThreadBinding,
    ) -> &SessionAgent {
        info!(
            "Spawning agent '{}' in session '{}' with binding {:?}",
            agent_id, self.id, binding
        );

        let agent =
            SessionAgent::new(agent_id.clone(), personality, binding, &self.primary_thread_id);

        self.agents.insert(agent_id.clone(), agent);
        self.last_activity = std::time::Instant::now();

        self.agents.get(&agent_id).unwrap()
    }

    /// Terminate an agent
    pub fn terminate_agent(&mut self, agent_id: &str) {
        if let Some(agent) = self.agents.get_mut(agent_id) {
            agent.mark_terminated();
            info!("Terminated agent '{}' in session '{}'", agent_id, self.id);
        }
    }

    /// Get an agent by ID
    pub fn get_agent(&self, agent_id: &str) -> Option<&SessionAgent> {
        self.agents.get(agent_id)
    }

    /// Get mutable agent by ID
    pub fn get_agent_mut(&mut self, agent_id: &str) -> Option<&mut SessionAgent> {
        self.agents.get_mut(agent_id)
    }

    /// Get all agents
    pub fn get_agents(&self) -> &HashMap<String, SessionAgent> {
        &self.agents
    }

    /// Get agents by thread binding
    pub fn get_agents_by_thread(&self, thread_id: &str) -> Vec<&SessionAgent> {
        self.agents
            .values()
            .filter(|a| a.thread_id == thread_id && a.is_active)
            .collect()
    }

    /// Get active agents
    pub fn get_active_agents(&self) -> Vec<&SessionAgent> {
        self.agents.values().filter(|a| a.is_active).collect()
    }

    /// Get shared context
    pub fn shared_context(&self) -> Arc<RwLock<HashMap<String, String>>> {
        self.shared_context.clone()
    }

    /// Get session status
    pub fn get_status(&self) -> SessionStatus {
        let active_agents: Vec<String> = self
            .agents
            .values()
            .filter(|a| a.is_active)
            .map(|a| a.id.clone())
            .collect();

        let thread_count = self
            .agents
            .values()
            .map(|a| &a.thread_id)
            .collect::<std::collections::HashSet<_>>()
            .len();

        SessionStatus {
            session_id: self.id.clone(),
            agent_count: self.agents.len(),
            active_agents,
            thread_count,
        }
    }

    /// Find best agent for a message based on intent
    pub fn find_agent_for_intent(&self, message: &str) -> Option<&SessionAgent> {
        let message_lower = message.to_lowercase();

        // Simple intent-based routing
        let intent_keywords: Vec<(&str, Vec<&str>)> = vec![
            ("code", vec!["code", "program", "debug", "fix", "error", "bug"]),
            ("review", vec!["review", "check", "audit", "analyze"]),
            ("lead", vec!["design", "architect", "plan", "coordinate"]),
            ("write", vec!["write", "document", "create", "draft"]),
        ];

        for (intent, keywords) in intent_keywords {
            if keywords.iter().any(|kw| message_lower.contains(kw)) {
                // Find an agent that can handle this intent
                return self.agents.values().find(|a| {
                    a.is_active
                        && a.status == AgentInstanceStatus::Ready
                        && a.personality.can_handle(intent)
                });
            }
        }

        // Fallback: return first ready agent
        self.agents
            .values()
            .find(|a| a.is_active && a.status == AgentInstanceStatus::Ready)
    }

    /// Check if session has timed out (no activity)
    pub fn is_timed_out(&self, timeout: std::time::Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }

    /// Cleanup terminated agents
    pub fn cleanup_terminated(&mut self) {
        self.agents
            .retain(|_, a| a.is_active || a.status != AgentInstanceStatus::Terminated);
    }
}

/// Session manager for all multi-agent sessions
#[derive(Debug, Default)]
pub struct SessionManager {
    /// Active sessions
    sessions: HashMap<String, MultiAgentSession>,
    /// Session timeout
    timeout: std::time::Duration,
}

impl SessionManager {
    /// Create new session manager
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            timeout: std::time::Duration::from_secs(3600), // 1 hour default
        }
    }

    /// Create a new session
    pub fn create_session(&mut self, session_id: String) -> mpsc::Sender<SessionMessage> {
        let (session, message_rx) = MultiAgentSession::new(session_id.clone());
        let sender = session.sender();

        // Spawn session processing task before moving session_id
        tokio::spawn(session_processing_task(session_id.clone(), message_rx));

        self.sessions.insert(session_id, session);

        sender
    }

    /// Get a session
    pub fn get_session(&self, session_id: &str) -> Option<&MultiAgentSession> {
        self.sessions.get(session_id)
    }

    /// Get mutable session
    pub fn get_session_mut(&mut self, session_id: &str) -> Option<&mut MultiAgentSession> {
        self.sessions.get_mut(session_id)
    }

    /// Terminate a session
    pub fn terminate_session(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            // Terminate all agents
            for agent_id in session.get_agents().keys().cloned().collect::<Vec<_>>() {
                session.terminate_agent(&agent_id);
            }
        }
        self.sessions.remove(session_id);
        info!("Terminated session '{}'", session_id);
    }

    /// Cleanup timed out sessions
    pub fn cleanup_timed_out(&mut self) {
        let timed_out: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.is_timed_out(self.timeout))
            .map(|(id, _)| id.clone())
            .collect();

        for session_id in timed_out {
            info!("Session '{}' timed out, terminating", session_id);
            self.terminate_session(&session_id);
        }
    }

    /// Get all session IDs
    pub fn list_sessions(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }

    /// Set session timeout
    pub fn set_timeout(&mut self, timeout: std::time::Duration) {
        self.timeout = timeout;
    }
}

/// Session processing task
async fn session_processing_task(
    session_id: String,
    mut message_rx: mpsc::Receiver<SessionMessage>,
) {
    info!("Session processing task started for {}", session_id);

    while let Some(msg) = message_rx.recv().await {
        match msg {
            SessionMessage::GetStatus { respond_to } => {
                // Get status would need access to session
                let status = SessionStatus {
                    session_id: session_id.clone(),
                    agent_count: 0,
                    active_agents: vec![],
                    thread_count: 0,
                };
                let _ = respond_to.send(status);
            }
            _ => {
                debug!("Session {} received message: {:?}", session_id, msg);
            }
        }
    }

    info!("Session processing task ended for {}", session_id);
}

use tokio::sync::oneshot;

/// Shared session manager
type SharedSessionManager = Arc<RwLock<SessionManager>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thread_binding_get_thread_id() {
        let parent = "parent-thread";

        let isolated = ThreadBinding::Isolated.get_thread_id(parent);
        assert!(isolated.starts_with("thread-"));
        assert_ne!(isolated, parent);

        let parent_binding = ThreadBinding::Parent.get_thread_id(parent);
        assert_eq!(parent_binding, parent);

        let existing = ThreadBinding::Existing("custom".to_string()).get_thread_id(parent);
        assert_eq!(existing, "custom");

        let shared = ThreadBinding::Shared.get_thread_id(parent);
        assert_eq!(shared, format!("shared-{}", parent));
    }

    #[test]
    fn test_session_agent_shares_context() {
        let parent = "parent-thread";

        let agent1 = SessionAgent::new(
            "agent1".to_string(),
            AgentPersonality::default(),
            ThreadBinding::Shared,
            parent,
        );

        let agent2 = SessionAgent::new(
            "agent2".to_string(),
            AgentPersonality::default(),
            ThreadBinding::Shared,
            parent,
        );

        let agent3 = SessionAgent::new(
            "agent3".to_string(),
            AgentPersonality::default(),
            ThreadBinding::Isolated,
            parent,
        );

        assert!(agent1.shares_context_with(&agent2));
        assert!(!agent1.shares_context_with(&agent3));
    }
}
