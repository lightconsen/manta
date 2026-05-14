//! Agent Router for the Inbound Pipeline
//!
//! Replaces the simple `resolve_agent_for_session` with a workspace-aware
//! multi-agent routing system.

use crate::channels::IncomingMessage;
use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Result of routing a message to an agent.
#[derive(Debug, Clone)]
pub struct RouteResult {
    /// The agent ID that should handle this message.
    pub agent_id: String,
    /// The workspace the agent belongs to (if any).
    pub workspace_id: Option<String>,
    /// Whether this is a new binding created on-the-fly.
    pub created_binding: bool,
}

/// Configuration for the agent router.
#[derive(Debug, Clone)]
pub struct AgentRouterConfig {
    /// Default agent ID when no other route matches.
    pub default_agent_id: String,
    /// Default workspace ID when no other route matches.
    pub default_workspace_id: Option<String>,
    /// Whether to create a new agent binding on-the-fly when none exists.
    pub auto_create_binding: bool,
}

impl Default for AgentRouterConfig {
    fn default() -> Self {
        Self {
            default_agent_id: "default".to_string(),
            default_workspace_id: None,
            auto_create_binding: true,
        }
    }
}

/// Workspace-aware multi-agent router.
///
/// Routes incoming messages to the appropriate agent based on:
/// - Session-to-agent binding table
/// - Explicit `@agent_name` mentions in message content
/// - Channel-specific defaults
/// - Workspace-specific defaults
///
/// This replaces `Gateway::resolve_agent_for_session`.
pub struct AgentRouter {
    config: AgentRouterConfig,
    /// session_id -> (agent_id, workspace_id)
    session_bindings: RwLock<HashMap<String, (String, Option<String>)>>,
    /// channel_name -> (agent_id, workspace_id)
    channel_defaults: RwLock<HashMap<String, (String, Option<String>)>>,
    /// workspace_id -> default_agent_id
    workspace_defaults: RwLock<HashMap<String, String>>,
}

impl Clone for AgentRouter {
    fn clone(&self) -> Self {
        // Since we can't clone RwLock contents without async, we create a new
        // router with the same config.  Bindings are repopulated on first use.
        Self::new(self.config.clone())
    }
}

impl AgentRouter {
    pub fn new(config: AgentRouterConfig) -> Self {
        Self {
            config,
            session_bindings: RwLock::new(HashMap::new()),
            channel_defaults: RwLock::new(HashMap::new()),
            workspace_defaults: RwLock::new(HashMap::new()),
        }
    }

    /// Route an incoming message to an agent.
    ///
    /// Resolution order:
    /// 1. Explicit `@agent_name` mention in message content
    /// 2. Existing session binding
    /// 3. Channel-specific default
    /// 4. Workspace-specific default
    /// 5. Global default agent
    pub async fn route(
        &self,
        message: &IncomingMessage,
        workspace_hint: Option<&str>,
    ) -> RouteResult {
        let session_id = message.conversation_id.0.clone();
        let content = message.content.trim();

        // 1. Check for explicit @agent_name mention
        if let Some(mentioned) = Self::extract_agent_mention(content) {
            info!("Explicit agent mention: {} for session {}", mentioned, session_id);
            return RouteResult {
                agent_id: mentioned,
                workspace_id: workspace_hint.map(String::from),
                created_binding: false,
            };
        }

        // 2. Check existing session binding
        {
            let bindings = self.session_bindings.read().await;
            if let Some((agent_id, workspace_id)) = bindings.get(&session_id) {
                debug!("Session {} bound to agent {}", session_id, agent_id);
                return RouteResult {
                    agent_id: agent_id.clone(),
                    workspace_id: workspace_id.clone(),
                    created_binding: false,
                };
            }
        }

        // 3. Check channel-specific default
        let channel_name = match &message.provenance {
            crate::channels::InputProvenance::ExternalUser { channel, .. } => Some(channel.clone()),
            _ => None,
        };

        if let Some(ch) = &channel_name {
            let defaults = self.channel_defaults.read().await;
            if let Some((agent_id, workspace_id)) = defaults.get(ch) {
                debug!("Channel {} default agent: {}", ch, agent_id);
                let result = RouteResult {
                    agent_id: agent_id.clone(),
                    workspace_id: workspace_id.clone(),
                    created_binding: true,
                };
                // Store binding for future messages
                drop(defaults);
                self.bind_session(&session_id, &result).await;
                return result;
            }
        }

        // 4. Check workspace-specific default
        if let Some(ws) = workspace_hint {
            let defaults = self.workspace_defaults.read().await;
            if let Some(agent_id) = defaults.get(ws) {
                debug!("Workspace {} default agent: {}", ws, agent_id);
                let result = RouteResult {
                    agent_id: agent_id.clone(),
                    workspace_id: Some(ws.to_string()),
                    created_binding: true,
                };
                drop(defaults);
                self.bind_session(&session_id, &result).await;
                return result;
            }
        }

        // 5. Fall back to global default
        debug!(
            "No binding found for session {}, using default agent {}",
            session_id, self.config.default_agent_id
        );
        let result = RouteResult {
            agent_id: self.config.default_agent_id.clone(),
            workspace_id: self.config.default_workspace_id.clone(),
            created_binding: true,
        };
        self.bind_session(&session_id, &result).await;
        result
    }

    /// Bind a session to a specific agent.
    pub async fn bind_session(&self, session_id: &str, route: &RouteResult) {
        let mut bindings = self.session_bindings.write().await;
        bindings.insert(
            session_id.to_string(),
            (route.agent_id.clone(), route.workspace_id.clone()),
        );
        info!(
            "Bound session {} to agent {} (workspace: {:?})",
            session_id, route.agent_id, route.workspace_id
        );
    }

    /// Unbind a session (e.g., on `/new` command).
    pub async fn unbind_session(&self, session_id: &str) {
        let mut bindings = self.session_bindings.write().await;
        if bindings.remove(session_id).is_some() {
            info!("Unbound session {}", session_id);
        }
    }

    /// Set the default agent for a channel.
    pub async fn set_channel_default(
        &self,
        channel: &str,
        agent_id: String,
        workspace_id: Option<String>,
    ) {
        let mut defaults = self.channel_defaults.write().await;
        defaults.insert(channel.to_string(), (agent_id, workspace_id));
    }

    /// Set the default agent for a workspace.
    pub async fn set_workspace_default(&self, workspace_id: &str, agent_id: String) {
        let mut defaults = self.workspace_defaults.write().await;
        defaults.insert(workspace_id.to_string(), agent_id);
    }

    /// List all active session bindings.
    pub async fn list_bindings(&self) -> HashMap<String, (String, Option<String>)> {
        let bindings = self.session_bindings.read().await;
        bindings.clone()
    }

    /// Resolve an agent by session ID only (used when the full IncomingMessage
    /// is not yet available, e.g. from the message queue processor).
    ///
    /// Checks session bindings and falls back to the global default.
    pub async fn resolve_by_session(&self, session_id: &str) -> RouteResult {
        // 1. Check existing session binding
        {
            let bindings = self.session_bindings.read().await;
            if let Some((agent_id, workspace_id)) = bindings.get(session_id) {
                debug!("Session {} bound to agent {}", session_id, agent_id);
                return RouteResult {
                    agent_id: agent_id.clone(),
                    workspace_id: workspace_id.clone(),
                    created_binding: false,
                };
            }
        }

        // 2. Fall back to global default
        debug!(
            "No binding found for session {}, using default agent {}",
            session_id, self.config.default_agent_id
        );
        let result = RouteResult {
            agent_id: self.config.default_agent_id.clone(),
            workspace_id: self.config.default_workspace_id.clone(),
            created_binding: true,
        };
        self.bind_session(session_id, &result).await;
        result
    }

    /// Extract an explicit `@agent_name` mention from message content.
    ///
    /// Looks for patterns like `@agent_name` at the start of the message.
    fn extract_agent_mention(content: &str) -> Option<String> {
        // Look for @agent_name at the very beginning
        if let Some(rest) = content.strip_prefix('@') {
            let name = rest.split_whitespace().next()?;
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                return Some(name.to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_route() {
        let router = AgentRouter::new(AgentRouterConfig::default());
        let msg = IncomingMessage::new("u1", "s1", "hello");
        let route = router.route(&msg, None).await;
        assert_eq!(route.agent_id, "default");
        assert!(route.created_binding);
    }

    #[tokio::test]
    async fn test_explicit_mention() {
        let router = AgentRouter::new(AgentRouterConfig::default());
        let msg = IncomingMessage::new("u1", "s1", "@coder write some rust");
        let route = router.route(&msg, None).await;
        assert_eq!(route.agent_id, "coder");
        assert!(!route.created_binding);
    }

    #[tokio::test]
    async fn test_session_binding_persists() {
        let router = AgentRouter::new(AgentRouterConfig::default());
        let msg = IncomingMessage::new("u1", "s1", "hello");

        // First message creates binding
        let route1 = router.route(&msg, None).await;
        assert!(route1.created_binding);

        // Second message uses existing binding
        let msg2 = IncomingMessage::new("u1", "s1", "again");
        let route2 = router.route(&msg2, None).await;
        assert!(!route2.created_binding);
        assert_eq!(route2.agent_id, route1.agent_id);
    }

    #[tokio::test]
    async fn test_workspace_default() {
        let router = AgentRouter::new(AgentRouterConfig::default());
        router
            .set_workspace_default("dev", "coder".to_string())
            .await;

        let msg = IncomingMessage::new("u1", "s1", "hello");
        let route = router.route(&msg, Some("dev")).await;
        assert_eq!(route.agent_id, "coder");
        assert_eq!(route.workspace_id, Some("dev".to_string()));
    }

    #[tokio::test]
    async fn test_unbind_session() {
        let router = AgentRouter::new(AgentRouterConfig::default());
        let msg = IncomingMessage::new("u1", "s1", "hello");

        let route1 = router.route(&msg, None).await;
        router.unbind_session("s1").await;

        let msg2 = IncomingMessage::new("u1", "s1", "again");
        let route2 = router.route(&msg2, None).await;
        // After unbind, a new binding is created (may be different agent)
        assert!(route2.created_binding);
    }
}
