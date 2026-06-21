//! Session envelope context for Syscity channels
//!
//! Provides `SessionEnvelopeContext` containing:
//! - `store_path` — Path to the session store file/directory
//! - `previous_timestamp` — Timestamp of the last message in the session
//!
//! Used for interval calculation (e.g., time since last activity),
//! session expiry checks, and session store location resolution.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Context passed with each message envelope for session management.
///
/// Allows the session system to calculate intervals (time since last message),
/// determine session store locations, and make expiry decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEnvelopeContext {
    /// Path to the session store directory.
    pub store_path: PathBuf,
    /// Timestamp of the previous message in this session, if any.
    pub previous_timestamp: Option<DateTime<Utc>>,
    /// When the session was created.
    pub session_created_at: DateTime<Utc>,
    /// Total message count in this session.
    pub message_count: u64,
    /// Custom metadata for the session envelope.
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, String>,
}

impl SessionEnvelopeContext {
    /// Create a new session envelope context.
    pub fn new(store_path: PathBuf) -> Self {
        Self {
            store_path,
            previous_timestamp: None,
            session_created_at: Utc::now(),
            message_count: 0,
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Set the store path.
    pub fn with_store_path(mut self, path: PathBuf) -> Self {
        self.store_path = path;
        self
    }

    /// Set the previous message timestamp.
    pub fn with_previous_timestamp(mut self, ts: DateTime<Utc>) -> Self {
        self.previous_timestamp = Some(ts);
        self
    }

    /// Set the session creation time.
    pub fn with_created_at(mut self, ts: DateTime<Utc>) -> Self {
        self.session_created_at = ts;
        self
    }

    /// Increment the message count.
    pub fn increment_message_count(&mut self) {
        self.message_count += 1;
    }

    /// Add custom metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Calculate the interval since the last message.
    ///
    /// Returns `None` if there is no previous message timestamp.
    pub fn interval_since_last(&self) -> Option<chrono::Duration> {
        self.previous_timestamp.map(|ts| Utc::now() - ts)
    }

    /// Calculate the session age (time since creation).
    pub fn session_age(&self) -> chrono::Duration {
        Utc::now() - self.session_created_at
    }

    /// Check if the session has exceeded the given idle timeout.
    pub fn is_idle(&self, timeout: chrono::Duration) -> bool {
        match self.interval_since_last() {
            Some(duration) => duration > timeout,
            None => false,
        }
    }

    /// Check if the session has exceeded the given max age.
    pub fn is_expired(&self, max_age: chrono::Duration) -> bool {
        self.session_age() > max_age
    }

    /// Get the number of messages per hour (average).
    pub fn messages_per_hour(&self) -> f64 {
        let age_hours = self.session_age().num_minutes() as f64 / 60.0;
        if age_hours > 0.0 {
            self.message_count as f64 / age_hours
        } else {
            self.message_count as f64
        }
    }
}

/// Manager for session envelope contexts across active sessions.
#[derive(Debug, Clone)]
pub struct SessionEnvelopeManager {
    /// Per-conversation envelope contexts.
    contexts: Arc<RwLock<std::collections::HashMap<String, SessionEnvelopeContext>>>,
    /// Default store path.
    default_store_path: PathBuf,
}

impl SessionEnvelopeManager {
    /// Create a new envelope manager with a default store path.
    pub fn new(default_store_path: PathBuf) -> Self {
        Self {
            contexts: Arc::new(RwLock::new(std::collections::HashMap::new())),
            default_store_path,
        }
    }

    /// Get or create an envelope context for a conversation.
    pub async fn get_or_create(&self, conversation_id: &str) -> SessionEnvelopeContext {
        let mut contexts = self.contexts.write().await;
        let store_path = self
            .default_store_path
            .join(sanitize_path_component(conversation_id));
        contexts
            .entry(conversation_id.to_string())
            .or_insert_with(|| SessionEnvelopeContext::new(store_path))
            .clone()
    }

    /// Get an existing envelope context.
    pub async fn get(&self, conversation_id: &str) -> Option<SessionEnvelopeContext> {
        let contexts = self.contexts.read().await;
        contexts.get(conversation_id).cloned()
    }

    /// Update the envelope context for a conversation after a message is
    /// processed.
    ///
    /// Sets the previous timestamp to now and increments the message count.
    pub async fn update_after_message(&self, conversation_id: &str) {
        let mut contexts = self.contexts.write().await;
        if let Some(ctx) = contexts.get_mut(conversation_id) {
            ctx.previous_timestamp = Some(Utc::now());
            ctx.message_count += 1;
        }
    }

    /// Remove an envelope context (e.g., on session end).
    pub async fn remove(&self, conversation_id: &str) {
        let mut contexts = self.contexts.write().await;
        contexts.remove(conversation_id);
    }

    /// List all active conversation IDs with their envelope contexts.
    pub async fn list_active(&self) -> Vec<(String, SessionEnvelopeContext)> {
        let contexts = self.contexts.read().await;
        contexts
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Count active sessions.
    pub async fn active_count(&self) -> usize {
        let contexts = self.contexts.read().await;
        contexts.len()
    }

    /// Clear all envelope contexts.
    pub async fn clear(&self) {
        let mut contexts = self.contexts.write().await;
        contexts.clear();
    }

    /// Remove idle sessions that exceed the given timeout.
    ///
    /// Returns the number of sessions removed.
    pub async fn evict_idle(&self, timeout: chrono::Duration) -> usize {
        let mut contexts = self.contexts.write().await;
        let now = Utc::now();
        let before = contexts.len();
        contexts.retain(|_, ctx| {
            ctx.previous_timestamp
                .map(|ts| now - ts <= timeout)
                .unwrap_or(true)
        });
        before - contexts.len()
    }

    /// Remove expired sessions that exceed the given max age.
    ///
    /// Returns the number of sessions removed.
    pub async fn evict_expired(&self, max_age: chrono::Duration) -> usize {
        let mut contexts = self.contexts.write().await;
        let now = Utc::now();
        let before = contexts.len();
        contexts.retain(|_, ctx| now - ctx.session_created_at <= max_age);
        before - contexts.len()
    }
}

/// Sanitize a string for use as a filesystem path component.
fn sanitize_path_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_session_envelope_context_new() {
        let ctx = SessionEnvelopeContext::new(PathBuf::from("/tmp/sessions"));
        assert_eq!(ctx.store_path, PathBuf::from("/tmp/sessions"));
        assert!(ctx.previous_timestamp.is_none());
        assert_eq!(ctx.message_count, 0);
    }

    #[test]
    fn test_interval_since_last() {
        let ctx = SessionEnvelopeContext::new(PathBuf::from("/tmp"));
        assert!(ctx.interval_since_last().is_none());

        let ctx = ctx.with_previous_timestamp(Utc::now());
        let interval = ctx.interval_since_last();
        assert!(interval.is_some());
        assert!(interval.unwrap().num_seconds() < 5); // just happened
    }

    #[test]
    fn test_session_age() {
        let ctx = SessionEnvelopeContext::new(PathBuf::from("/tmp"));
        let age = ctx.session_age();
        assert!(age.num_seconds() < 5);
    }

    #[test]
    fn test_is_idle() {
        let ctx = SessionEnvelopeContext::new(PathBuf::from("/tmp"))
            .with_previous_timestamp(Utc::now() - chrono::Duration::hours(2));
        assert!(ctx.is_idle(chrono::Duration::hours(1)));
        assert!(!ctx.is_idle(chrono::Duration::hours(3)));
    }

    #[test]
    fn test_is_expired() {
        let ctx = SessionEnvelopeContext::new(PathBuf::from("/tmp"));
        // Not expired since it was just created
        assert!(!ctx.is_expired(chrono::Duration::seconds(1)));

        // Use a context created 2 days ago
        let old_ctx = SessionEnvelopeContext {
            store_path: PathBuf::from("/tmp"),
            previous_timestamp: None,
            session_created_at: Utc::now() - chrono::Duration::days(2),
            message_count: 0,
            metadata: std::collections::HashMap::new(),
        };
        assert!(old_ctx.is_expired(chrono::Duration::days(1)));
    }

    #[test]
    fn test_messages_per_hour() {
        let mut ctx = SessionEnvelopeContext::new(PathBuf::from("/tmp"));
        ctx.session_created_at = Utc::now() - chrono::Duration::hours(2);
        ctx.message_count = 10;
        let mph = ctx.messages_per_hour();
        assert!((mph - 5.0).abs() < 0.1); // 10 messages / 2 hours = 5
    }

    #[test]
    fn test_increment_message_count() {
        let mut ctx = SessionEnvelopeContext::new(PathBuf::from("/tmp"));
        assert_eq!(ctx.message_count, 0);
        ctx.increment_message_count();
        assert_eq!(ctx.message_count, 1);
    }

    #[test]
    fn test_metadata() {
        let ctx =
            SessionEnvelopeContext::new(PathBuf::from("/tmp")).with_metadata("channel", "telegram");
        assert_eq!(ctx.metadata.get("channel").unwrap(), "telegram");
    }

    #[tokio::test]
    async fn test_envelope_manager_get_or_create() {
        let manager = SessionEnvelopeManager::new(PathBuf::from("/tmp/sessions"));
        let ctx = manager.get_or_create("conv_123").await;
        assert_eq!(ctx.store_path, PathBuf::from("/tmp/sessions/conv_123"));
    }

    #[tokio::test]
    async fn test_envelope_manager_update_after_message() {
        let manager = SessionEnvelopeManager::new(PathBuf::from("/tmp"));
        let initial = manager.get_or_create("conv_1").await;
        assert_eq!(initial.message_count, 0);
        assert!(initial.previous_timestamp.is_none());

        manager.update_after_message("conv_1").await;
        let updated = manager.get("conv_1").await.unwrap();
        assert_eq!(updated.message_count, 1);
        assert!(updated.previous_timestamp.is_some());
    }

    #[tokio::test]
    async fn test_envelope_manager_evict_idle() {
        let manager = SessionEnvelopeManager::new(PathBuf::from("/tmp"));
        manager.get_or_create("active").await;
        manager.get_or_create("idle").await;

        // Set idle conv to be idle
        {
            let mut contexts = manager.contexts.write().await;
            if let Some(ctx) = contexts.get_mut("idle") {
                ctx.previous_timestamp = Some(Utc::now() - chrono::Duration::hours(2));
            }
        }

        let evicted = manager.evict_idle(chrono::Duration::hours(1)).await;
        assert_eq!(evicted, 1);
        assert!(manager.get("idle").await.is_none());
        assert!(manager.get("active").await.is_some());
    }

    #[tokio::test]
    async fn test_envelope_manager_evict_expired() {
        let manager = SessionEnvelopeManager::new(PathBuf::from("/tmp"));
        manager.get_or_create("fresh").await;
        manager.get_or_create("old").await;

        // Set old conv to be old
        {
            let mut contexts = manager.contexts.write().await;
            if let Some(ctx) = contexts.get_mut("old") {
                ctx.session_created_at = Utc::now() - chrono::Duration::days(10);
            }
        }

        let evicted = manager.evict_expired(chrono::Duration::days(7)).await;
        assert_eq!(evicted, 1);
    }

    #[tokio::test]
    async fn test_envelope_manager_remove() {
        let manager = SessionEnvelopeManager::new(PathBuf::from("/tmp"));
        manager.get_or_create("conv_1").await;
        assert!(manager.get("conv_1").await.is_some());
        manager.remove("conv_1").await;
        assert!(manager.get("conv_1").await.is_none());
    }

    #[tokio::test]
    async fn test_envelope_manager_active_count() {
        let manager = SessionEnvelopeManager::new(PathBuf::from("/tmp"));
        assert_eq!(manager.active_count().await, 0);
        manager.get_or_create("a").await;
        manager.get_or_create("b").await;
        assert_eq!(manager.active_count().await, 2);
    }

    #[test]
    fn test_sanitize_path() {
        assert_eq!(sanitize_path_component("conv_123"), "conv_123");
        assert_eq!(sanitize_path_component("user@host"), "user_host");
        assert_eq!(sanitize_path_component("../etc"), ".._etc");
    }
}
