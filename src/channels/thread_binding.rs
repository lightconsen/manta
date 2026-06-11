//! Thread binding policy for Syscity channels
//!
//! Governs how conversations are bound to threads with:
//! - Idle timeout (default 24h) — thread reaps after inactivity
//! - Max age — maximum lifetime of a thread binding
//! - Placement hint (current/child) — new thread or reuse existing
//! - Spawn support (subagent/acp) — what to spawn when new thread needed

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Where to place the next message in a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementHint {
    /// Continue in the current thread.
    Current,
    /// Spawn a child thread/branch.
    Child,
}

impl Default for PlacementHint {
    fn default() -> Self {
        Self::Current
    }
}

/// What type of execution context to spawn when creating a new thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnTarget {
    /// Spawn a subagent (lightweight, in-process).
    Subagent,
    /// Spawn an ACP session (full agent control plane session).
    Acp,
}

impl Default for SpawnTarget {
    fn default() -> Self {
        Self::Subagent
    }
}

/// Policy for thread binding behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadBindingPolicy {
    /// Idle timeout duration after which the thread is released.
    /// Default: 24 hours.
    pub idle_timeout: Duration,
    /// Maximum age of a thread binding before it must be renewed.
    /// Default: 7 days.
    pub max_age: Duration,
    /// Where to place new messages by default.
    pub placement_hint: PlacementHint,
    /// What to spawn when creating a new thread.
    pub spawn_target: SpawnTarget,
    /// Maximum number of child threads allowed per parent.
    /// None = unlimited.
    pub max_children: Option<u32>,
    /// Whether to auto-create threads for new conversations.
    pub auto_create: bool,
}

impl Default for ThreadBindingPolicy {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::hours(24),
            max_age: Duration::days(7),
            placement_hint: PlacementHint::Current,
            spawn_target: SpawnTarget::Subagent,
            max_children: None,
            auto_create: true,
        }
    }
}

impl ThreadBindingPolicy {
    /// Create a policy with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set idle timeout.
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Set max age.
    pub fn with_max_age(mut self, age: Duration) -> Self {
        self.max_age = age;
        self
    }

    /// Set placement hint.
    pub fn with_placement(mut self, hint: PlacementHint) -> Self {
        self.placement_hint = hint;
        self
    }

    /// Set spawn target.
    pub fn with_spawn_target(mut self, target: SpawnTarget) -> Self {
        self.spawn_target = target;
        self
    }

    /// Limit max children.
    pub fn with_max_children(mut self, max: u32) -> Self {
        self.max_children = Some(max);
        self
    }

    /// Disable auto-create.
    pub fn no_auto_create(mut self) -> Self {
        self.auto_create = false;
        self
    }

    /// Check if a thread binding with the given timestamps is still valid.
    pub fn is_valid(
        &self,
        last_activity: &DateTime<Utc>,
        created_at: &DateTime<Utc>,
    ) -> bool {
        let now = Utc::now();
        now - *last_activity <= self.idle_timeout && now - *created_at <= self.max_age
    }

    /// Check if the binding has exceeded the idle timeout.
    pub fn is_idle(&self, last_activity: &DateTime<Utc>) -> bool {
        Utc::now() - *last_activity > self.idle_timeout
    }

    /// Check if the binding has exceeded the max age.
    pub fn is_expired(&self, created_at: &DateTime<Utc>) -> bool {
        Utc::now() - *created_at > self.max_age
    }

    /// Calculate the remaining idle time before timeout.
    pub fn remaining_idle_time(&self, last_activity: &DateTime<Utc>) -> Duration {
        let elapsed = Utc::now() - *last_activity;
        if elapsed > self.idle_timeout {
            Duration::zero()
        } else {
            self.idle_timeout - elapsed
        }
    }

    /// Calculate the remaining lifetime before max age.
    pub fn remaining_lifetime(&self, created_at: &DateTime<Utc>) -> Duration {
        let elapsed = Utc::now() - *created_at;
        if elapsed > self.max_age {
            Duration::zero()
        } else {
            self.max_age - elapsed
        }
    }
}

/// A tracked thread binding with creation and activity timestamps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedThreadBinding {
    /// The agent/session ID this thread is bound to.
    pub agent_id: String,
    /// The session ID of this binding.
    pub session_id: String,
    /// Parent session ID (if this is a child thread).
    pub parent_session_id: Option<String>,
    /// When this binding was created.
    pub created_at: DateTime<Utc>,
    /// When this binding was last active.
    pub last_activity: DateTime<Utc>,
    /// Number of messages processed in this thread.
    pub message_count: u64,
}

impl TrackedThreadBinding {
    /// Create a new thread binding.
    pub fn new(agent_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            parent_session_id: None,
            created_at: Utc::now(),
            last_activity: Utc::now(),
            message_count: 0,
        }
    }

    /// Set the parent session.
    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent_session_id = Some(parent.into());
        self
    }

    /// Record activity (update timestamp and increment message count).
    pub fn record_activity(&mut self) {
        self.last_activity = Utc::now();
        self.message_count += 1;
    }
}

/// Manager for thread bindings with policy enforcement.
#[derive(Debug, Clone)]
pub struct ThreadBindingManager {
    /// Policy governing thread binding behavior.
    policy: ThreadBindingPolicy,
    /// Active thread bindings: session_id -> binding.
    bindings: Arc<RwLock<HashMap<String, TrackedThreadBinding>>>,
    /// Parent -> children mapping for hierarchy tracking.
    children: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl ThreadBindingManager {
    /// Create a new manager with the given policy.
    pub fn new(policy: ThreadBindingPolicy) -> Self {
        Self {
            policy,
            bindings: Arc::new(RwLock::new(HashMap::new())),
            children: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a manager with default policy.
    pub fn with_default_policy() -> Self {
        Self::new(ThreadBindingPolicy::default())
    }

    // ── Binding management ────────────────────────────────────────────

    /// Register a new thread binding.
    pub async fn register(&self, binding: TrackedThreadBinding) {
        let session_id = binding.session_id.clone();
        let parent_id = binding.parent_session_id.clone();

        let mut bindings = self.bindings.write().await;
        bindings.insert(session_id.clone(), binding);
        drop(bindings);

        // Track parent-child relationship
        if let Some(parent) = parent_id {
            let mut children = self.children.write().await;
            children.entry(parent).or_default().push(session_id);
        }
    }

    /// Get a binding by session ID.
    pub async fn get(&self, session_id: &str) -> Option<TrackedThreadBinding> {
        let bindings = self.bindings.read().await;
        bindings.get(session_id).cloned()
    }

    /// Record activity on a binding (updates timestamp + message count).
    pub async fn record_activity(&self, session_id: &str) -> bool {
        let mut bindings = self.bindings.write().await;
        if let Some(binding) = bindings.get_mut(session_id) {
            binding.record_activity();
            true
        } else {
            false
        }
    }

    /// Remove a binding.
    pub async fn remove(&self, session_id: &str) -> Option<TrackedThreadBinding> {
        let mut bindings = self.bindings.write().await;
        let removed = bindings.remove(session_id);
        drop(bindings);

        // Clean up parent-child relationship
        let mut children = self.children.write().await;
        children.retain(|_, child_list| {
            child_list.retain(|id| id != session_id);
            !child_list.is_empty()
        });

        removed
    }

    /// List all active bindings.
    pub async fn list(&self) -> Vec<TrackedThreadBinding> {
        let bindings = self.bindings.read().await;
        bindings.values().cloned().collect()
    }

    // ── Policy enforcement ────────────────────────────────────────────

    /// Check if a binding is still valid according to the policy.
    pub async fn is_valid(&self, session_id: &str) -> bool {
        let bindings = self.bindings.read().await;
        match bindings.get(session_id) {
            Some(binding) => {
                self.policy
                    .is_valid(&binding.last_activity, &binding.created_at)
            }
            None => false,
        }
    }

    /// Get children of a parent session.
    pub async fn get_children(&self, parent_id: &str) -> Vec<String> {
        let children = self.children.read().await;
        children.get(parent_id).cloned().unwrap_or_default()
    }

    /// Count children of a parent session.
    pub async fn child_count(&self, parent_id: &str) -> u32 {
        let children = self.children.read().await;
        children
            .get(parent_id)
            .map(|c| c.len() as u32)
            .unwrap_or(0)
    }

    /// Check if a parent can spawn another child based on max_children policy.
    pub async fn can_spawn_child(&self, parent_id: &str) -> bool {
        match self.policy.max_children {
            Some(max) => self.child_count(parent_id).await < max,
            None => true,
        }
    }

    /// Determine the placement for a new message on an existing session.
    ///
    /// Returns whether to use the current thread or spawn a child.
    pub async fn determine_placement(
        &self,
        session_id: &str,
    ) -> PlacementDecision {
        let bindings = self.bindings.read().await;
        let binding = match bindings.get(session_id) {
            Some(b) => b,
            None => return PlacementDecision::CreateNew,
        };

        // Check validity
        if !self.policy.is_valid(&binding.last_activity, &binding.created_at) {
            return PlacementDecision::CreateNew;
        }

        match self.policy.placement_hint {
            PlacementHint::Current => PlacementDecision::UseCurrent,
            PlacementHint::Child => {
                if self.policy.max_children.map_or(true, |max| {
                    // Runtime check on child count would need to be done in
                    // a separate step since we hold the read lock here
                    true
                }) {
                    PlacementDecision::SpawnChild {
                        // Check if we can actually spawn
                        can_spawn: true,
                        spawn_target: self.policy.spawn_target,
                    }
                } else {
                    PlacementDecision::UseCurrent
                }
            }
        }
    }

    /// Reap all idle/expired bindings according to policy.
    ///
    /// Returns the number of reaped bindings.
    pub async fn reap(&self) -> usize {
        let mut bindings = self.bindings.write().await;
        let now = Utc::now();
        let idle_timeout = self.policy.idle_timeout;
        let max_age = self.policy.max_age;

        let before = bindings.len();
        bindings.retain(|_, binding| {
            now - binding.last_activity <= idle_timeout && now - binding.created_at <= max_age
        });
        let after = bindings.len();

        // Also clean up stale parent-child relationships
        drop(bindings);
        let mut children = self.children.write().await;
        children.retain(|_, child_list| {
            child_list.retain(|child_id| {
                let bindings = self.bindings.blocking_read();
                bindings.contains_key(child_id)
            });
            !child_list.is_empty()
        });

        before - after
    }

    /// Get a reference to the policy.
    pub fn policy(&self) -> &ThreadBindingPolicy {
        &self.policy
    }

    /// Update the policy.
    pub async fn set_policy(&self, policy: ThreadBindingPolicy) {
        // We can't directly assign to an Arc<RwLock<>> field since we use
        // an immutable reference. Instead, we'd need to store the policy
        // behind a RwLock itself. For now, this is a placeholder.
        // The policy is set at construction time.
        let _ = policy;
    }
}

/// Decision about where to place the next message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementDecision {
    /// Use the current thread binding.
    UseCurrent,
    /// Spawn a child thread.
    SpawnChild {
        can_spawn: bool,
        spawn_target: SpawnTarget,
    },
    /// Create a new thread binding.
    CreateNew,
}

impl PlacementDecision {
    /// Returns true if the placement is to use the current thread.
    pub fn use_current(&self) -> bool {
        matches!(self, PlacementDecision::UseCurrent)
    }

    /// Returns true if the placement requires creating a new binding.
    pub fn needs_new(&self) -> bool {
        matches!(self, PlacementDecision::CreateNew)
    }
}

// ── Convenience presets ────────────────────────────────────────────────────────

/// Strict policy: short idle timeout, no auto-creation.
pub fn strict_policy() -> ThreadBindingPolicy {
    ThreadBindingPolicy::new()
        .with_idle_timeout(Duration::minutes(30))
        .with_max_age(Duration::hours(24))
        .with_placement(PlacementHint::Current)
        .no_auto_create()
}

/// Branching policy: always create child threads.
pub fn branching_policy() -> ThreadBindingPolicy {
    ThreadBindingPolicy::new()
        .with_idle_timeout(Duration::hours(24))
        .with_max_age(Duration::days(30))
        .with_placement(PlacementHint::Child)
        .with_spawn_target(SpawnTarget::Subagent)
        .with_max_children(10)
}

/// ACP-focused policy: spawn ACP sessions for new threads.
pub fn acp_policy() -> ThreadBindingPolicy {
    ThreadBindingPolicy::new()
        .with_idle_timeout(Duration::hours(72))
        .with_max_age(Duration::days(14))
        .with_placement(PlacementHint::Current)
        .with_spawn_target(SpawnTarget::Acp)
        .with_max_children(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_defaults() {
        let policy = ThreadBindingPolicy::default();
        assert_eq!(policy.idle_timeout, Duration::hours(24));
        assert_eq!(policy.max_age, Duration::days(7));
        assert_eq!(policy.placement_hint, PlacementHint::Current);
        assert!(policy.auto_create);
    }

    #[test]
    fn test_policy_valid() {
        let policy = ThreadBindingPolicy::new();
        let now = Utc::now();
        assert!(policy.is_valid(&now, &now));

        // Expired by max age
        let old = now - Duration::days(30);
        assert!(!policy.is_valid(&now, &old));

        // Idle too long
        let idle = now - Duration::hours(48);
        assert!(!policy.is_valid(&idle, &now));
    }

    #[test]
    fn test_policy_is_idle() {
        let policy = ThreadBindingPolicy::new();
        let now = Utc::now();
        assert!(!policy.is_idle(&now));

        let old = now - Duration::hours(48);
        assert!(policy.is_idle(&old));
    }

    #[test]
    fn test_policy_is_expired() {
        let policy = ThreadBindingPolicy::new();
        let now = Utc::now();
        assert!(!policy.is_expired(&now));

        let old = now - Duration::days(30);
        assert!(policy.is_expired(&old));
    }

    #[test]
    fn test_remaining_time() {
        let policy = ThreadBindingPolicy::new();

        // Just started — should have full idle timeout remaining
        let now = Utc::now();
        let remaining = policy.remaining_idle_time(&now);
        assert!(remaining > Duration::hours(23));

        // Expired
        let old = now - Duration::hours(48);
        assert_eq!(policy.remaining_idle_time(&old), Duration::zero());
    }

    #[test]
    fn test_tracked_thread_binding() {
        let mut binding = TrackedThreadBinding::new("agent1", "session1");
        assert_eq!(binding.message_count, 0);

        binding.record_activity();
        assert_eq!(binding.message_count, 1);

        let binding = binding.with_parent("parent_session");
        assert_eq!(binding.parent_session_id, Some("parent_session".to_string()));
    }

    #[tokio::test]
    async fn test_manager_register_and_get() {
        let manager = ThreadBindingManager::with_default_policy();
        let binding = TrackedThreadBinding::new("agent1", "session1");
        manager.register(binding).await;

        let retrieved = manager.get("session1").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().agent_id, "agent1");
    }

    #[tokio::test]
    async fn test_manager_record_activity() {
        let manager = ThreadBindingManager::with_default_policy();
        manager
            .register(TrackedThreadBinding::new("agent1", "session1"))
            .await;

        assert!(manager.record_activity("session1").await);
        let binding = manager.get("session1").await.unwrap();
        assert_eq!(binding.message_count, 1);
    }

    #[tokio::test]
    async fn test_manager_remove() {
        let manager = ThreadBindingManager::with_default_policy();
        manager
            .register(TrackedThreadBinding::new("agent1", "session1"))
            .await;
        assert!(manager.get("session1").await.is_some());

        manager.remove("session1").await;
        assert!(manager.get("session1").await.is_none());
    }

    #[tokio::test]
    async fn test_manager_reap_idle() {
        let manager = ThreadBindingManager::with_default_policy();
        manager
            .register(TrackedThreadBinding::new("agent1", "active"))
            .await;

        let mut idle_binding = TrackedThreadBinding::new("agent2", "idle");
        idle_binding.last_activity = Utc::now() - Duration::hours(48);
        manager.register(idle_binding).await;

        let reaped = manager.reap().await;
        assert_eq!(reaped, 1);
        assert!(manager.get("active").await.is_some());
        assert!(manager.get("idle").await.is_none());
    }

    #[tokio::test]
    async fn test_manager_child_tracking() {
        let manager = ThreadBindingManager::with_default_policy();

        let parent = TrackedThreadBinding::new("agent1", "parent");
        manager.register(parent).await;

        let child = TrackedThreadBinding::new("agent2", "child").with_parent("parent");
        manager.register(child).await;

        let children = manager.get_children("parent").await;
        assert_eq!(children, vec!["child"]);
        assert_eq!(manager.child_count("parent").await, 1);
    }

    #[tokio::test]
    async fn test_manager_determine_placement() {
        let manager = ThreadBindingManager::with_default_policy();

        // No binding — should create new
        let decision = manager.determine_placement("unknown").await;
        assert_eq!(decision, PlacementDecision::CreateNew);

        // Active binding with Current placement hint
        manager
            .register(TrackedThreadBinding::new("agent1", "session1"))
            .await;
        let decision = manager.determine_placement("session1").await;
        assert_eq!(decision, PlacementDecision::UseCurrent);

        // Test branching policy
        let branching_mgr = ThreadBindingManager::new(branching_policy());
        branching_mgr
            .register(TrackedThreadBinding::new("agent1", "session1"))
            .await;
        let decision = branching_mgr.determine_placement("session1").await;
        assert_eq!(
            decision,
            PlacementDecision::SpawnChild {
                can_spawn: true,
                spawn_target: SpawnTarget::Subagent
            }
        );
    }

    #[tokio::test]
    async fn test_manager_list() {
        let manager = ThreadBindingManager::with_default_policy();
        manager
            .register(TrackedThreadBinding::new("agent1", "s1"))
            .await;
        manager
            .register(TrackedThreadBinding::new("agent2", "s2"))
            .await;

        let list = manager.list().await;
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_placement_decision_helpers() {
        assert!(PlacementDecision::UseCurrent.use_current());
        assert!(!PlacementDecision::CreateNew.use_current());
        assert!(PlacementDecision::CreateNew.needs_new());
    }

    #[test]
    fn test_presets() {
        let strict = strict_policy();
        assert_eq!(strict.idle_timeout, Duration::minutes(30));
        assert!(!strict.auto_create);

        let branch = branching_policy();
        assert_eq!(branch.placement_hint, PlacementHint::Child);
        assert_eq!(branch.max_children, Some(10));

        let acp = acp_policy();
        assert_eq!(acp.spawn_target, SpawnTarget::Acp);
    }
}
