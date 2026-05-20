//! Agent Router for the Inbound Pipeline
//!
//! Replaces the simple `resolve_agent_for_session` with a workspace-aware
//! multi-agent routing system.

use crate::channels::IncomingMessage;
use async_trait::async_trait;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

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

/// Persistent storage for session-to-agent bindings.
#[async_trait]
pub trait BindingStore: Send + Sync {
    /// Load all stored bindings into memory.
    async fn load_bindings(&self) -> crate::Result<HashMap<String, (String, Option<String>)>>;
    /// Persist a single binding.
    async fn save_binding(
        &self,
        session_id: &str,
        agent_id: &str,
        workspace_id: Option<&str>,
    ) -> crate::Result<()>;
    /// Remove a persisted binding.
    async fn remove_binding(&self, session_id: &str) -> crate::Result<()>;
}

/// In-memory binding store (default, non-persistent).
pub struct InMemoryBindingStore {
    data: Arc<RwLock<HashMap<String, (String, Option<String>)>>>,
}

impl InMemoryBindingStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryBindingStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BindingStore for InMemoryBindingStore {
    async fn load_bindings(&self) -> crate::Result<HashMap<String, (String, Option<String>)>> {
        let data = self.data.read().await;
        Ok(data.clone())
    }

    async fn save_binding(
        &self,
        session_id: &str,
        agent_id: &str,
        workspace_id: Option<&str>,
    ) -> crate::Result<()> {
        let mut data = self.data.write().await;
        data.insert(session_id.to_string(), (agent_id.to_string(), workspace_id.map(String::from)));
        Ok(())
    }

    async fn remove_binding(&self, session_id: &str) -> crate::Result<()> {
        let mut data = self.data.write().await;
        data.remove(session_id);
        Ok(())
    }
}

/// SQLite-backed binding store for persistence across restarts.
pub struct SqliteBindingStore {
    pool: sqlx::Pool<sqlx::Sqlite>,
}

impl SqliteBindingStore {
    pub async fn new(pool: sqlx::Pool<sqlx::Sqlite>) -> crate::Result<Self> {
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    async fn ensure_schema(&self) -> crate::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS session_bindings (
                session_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                workspace_id TEXT,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create session_bindings table".to_string(),
            details: e.to_string(),
        })?;
        Ok(())
    }
}

#[async_trait]
impl BindingStore for SqliteBindingStore {
    async fn load_bindings(&self) -> crate::Result<HashMap<String, (String, Option<String>)>> {
        let rows = sqlx::query("SELECT session_id, agent_id, workspace_id FROM session_bindings")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to load session bindings".to_string(),
                details: e.to_string(),
            })?;

        let mut bindings = HashMap::new();
        for row in rows {
            let session_id: String = row.try_get("session_id").unwrap_or_default();
            let agent_id: String = row.try_get("agent_id").unwrap_or_default();
            let workspace_id: Option<String> = row.try_get("workspace_id").ok();
            bindings.insert(session_id, (agent_id, workspace_id));
        }
        Ok(bindings)
    }

    async fn save_binding(
        &self,
        session_id: &str,
        agent_id: &str,
        workspace_id: Option<&str>,
    ) -> crate::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO session_bindings (session_id, agent_id, workspace_id, created_at)
            VALUES (?1, ?2, ?3, unixepoch())
            ON CONFLICT(session_id) DO UPDATE SET
                agent_id = excluded.agent_id,
                workspace_id = excluded.workspace_id,
                created_at = excluded.created_at
            "#,
        )
        .bind(session_id)
        .bind(agent_id)
        .bind(workspace_id)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to save session binding".to_string(),
            details: e.to_string(),
        })?;
        Ok(())
    }

    async fn remove_binding(&self, session_id: &str) -> crate::Result<()> {
        sqlx::query("DELETE FROM session_bindings WHERE session_id = ?1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to remove session binding".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
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
    /// Optional persistent binding store
    binding_store: Option<Arc<dyn BindingStore>>,
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
            binding_store: None,
        }
    }

    /// Attach a persistent binding store.
    pub fn with_binding_store(mut self, store: Arc<dyn BindingStore>) -> Self {
        self.binding_store = Some(store);
        self
    }

    /// Load persisted bindings into memory.
    pub async fn load_bindings(&self) -> crate::Result<()> {
        if let Some(store) = &self.binding_store {
            match store.load_bindings().await {
                Ok(bindings) => {
                    let mut session_bindings = self.session_bindings.write().await;
                    *session_bindings = bindings;
                    info!("Loaded {} persisted session bindings", session_bindings.len());
                }
                Err(e) => {
                    warn!("Failed to load persisted bindings: {}", e);
                }
            }
        }
        Ok(())
    }

    /// Derive a stable session key from channel and user_id.
    ///
    /// OpenClaw-style normalization: `{channel}:{user_id}`
    pub fn derive_session_key(channel: &str, user_id: &str) -> String {
        format!("{}:{}", channel, user_id)
    }

    /// Route an incoming message to an agent.
    ///
    /// Resolution order:
    /// 1. Explicit `@agent_name` mention in message content
    /// 2. Existing session binding (by conversation_id)
    /// 2b. Existing session binding (by derived `{channel}:{user_id}` key)
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

        // 2. Check existing session binding (by conversation_id)
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

        // 2b. Check existing session binding by derived `{channel}:{user_id}` key
        let channel_name = match &message.provenance {
            crate::channels::InputProvenance::ExternalUser { channel, .. } => {
                Some(channel.as_str())
            }
            _ => None,
        };
        let user_id = message.user_id.0.as_str();

        if let Some(ch) = channel_name {
            let derived_key = Self::derive_session_key(ch, user_id);
            if derived_key != session_id {
                let bindings = self.session_bindings.read().await;
                if let Some((agent_id, workspace_id)) = bindings.get(&derived_key) {
                    debug!(
                        "Session {} resolved via derived key {} to agent {}",
                        session_id, derived_key, agent_id
                    );
                    return RouteResult {
                        agent_id: agent_id.clone(),
                        workspace_id: workspace_id.clone(),
                        created_binding: false,
                    };
                }
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
        bindings
            .insert(session_id.to_string(), (route.agent_id.clone(), route.workspace_id.clone()));
        drop(bindings);

        if let Some(store) = &self.binding_store {
            if let Err(e) = store
                .save_binding(session_id, &route.agent_id, route.workspace_id.as_deref())
                .await
            {
                warn!("Failed to persist binding for session {}: {}", session_id, e);
            }
        }

        info!(
            "Bound session {} to agent {} (workspace: {:?})",
            session_id, route.agent_id, route.workspace_id
        );
    }

    /// Unbind a session (e.g., on `/new` command).
    pub async fn unbind_session(&self, session_id: &str) {
        let mut bindings = self.session_bindings.write().await;
        let removed = bindings.remove(session_id).is_some();
        drop(bindings);

        if removed {
            if let Some(store) = &self.binding_store {
                if let Err(e) = store.remove_binding(session_id).await {
                    warn!("Failed to remove persisted binding for session {}: {}", session_id, e);
                }
            }
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
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
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

        let _route1 = router.route(&msg, None).await;
        router.unbind_session("s1").await;

        let msg2 = IncomingMessage::new("u1", "s1", "again");
        let route2 = router.route(&msg2, None).await;
        // After unbind, a new binding is created (may be different agent)
        assert!(route2.created_binding);
    }
}
