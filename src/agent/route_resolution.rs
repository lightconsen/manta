//! Route Resolution
//!
//! Replaces simple HashMap<session_id, agent_id> routing with a
//! multi-dimensional resolution system ts`.
//!
//! Resolution dimensions:
//! - peer (user ID)
//! - guild / team (group context)
//! - account (bot account)
//! - channel (channel name)
//! - scope (dm / channel / thread)
//! - role (user role for role-based routing)
//!
//! Bindings are cached after first evaluation to avoid repeated computation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{debug, info};

/// Scope of a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConversationScope {
    /// Direct message between user and bot.
    Dm,
    /// Group channel (public or private).
    Channel,
    /// Thread within a channel.
    Thread,
}

impl std::fmt::Display for ConversationScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversationScope::Dm => write!(f, "dm"),
            ConversationScope::Channel => write!(f, "channel"),
            ConversationScope::Thread => write!(f, "thread"),
        }
    }
}

/// Input dimensions for route resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteResolution {
    /// The peer (user) ID.
    pub peer: String,
    /// Optional guild / team ID (group context).
    pub guild: Option<String>,
    /// Optional team override.
    pub team: Option<String>,
    /// Bot account ID (for multi-account setups).
    pub account: String,
    /// Channel name or ID.
    pub channel: String,
    /// Conversation scope.
    pub scope: ConversationScope,
    /// Whether this is a role-based route.
    pub role_based: bool,
    /// Optional user role (for role-based routing).
    pub role: Option<String>,
}

impl RouteResolution {
    /// Create a new route resolution from basic parameters.
    pub fn new(
        peer: impl Into<String>,
        channel: impl Into<String>,
        scope: ConversationScope,
    ) -> Self {
        Self {
            peer: peer.into(),
            guild: None,
            team: None,
            account: "default".to_string(),
            channel: channel.into(),
            scope,
            role_based: false,
            role: None,
        }
    }

    /// Set the guild.
    pub fn with_guild(mut self, guild: impl Into<String>) -> Self {
        self.guild = Some(guild.into());
        self
    }

    /// Set the account.
    pub fn with_account(mut self, account: impl Into<String>) -> Self {
        self.account = account.into();
        self
    }

    /// Set the team.
    pub fn with_team(mut self, team: impl Into<String>) -> Self {
        self.team = Some(team.into());
        self
    }

    /// Mark as role-based with a role.
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role_based = true;
        self.role = Some(role.into());
        self
    }

    /// Compute a cache key from the resolution.
    pub fn cache_key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.peer,
            self.guild.as_deref().unwrap_or("-"),
            self.team.as_deref().unwrap_or("-"),
            self.account,
            self.channel,
            self.scope,
            self.role.as_deref().unwrap_or("-"),
        )
    }

    /// Compute a session-scoped key (ignores role for caching).
    pub fn session_key(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.peer,
            self.guild.as_deref().unwrap_or("-"),
            self.channel,
            self.scope,
        )
    }
}

/// A fully-resolved binding: which thread, agent, and mode to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBinding {
    /// Thread ID for this conversation.
    pub thread_id: String,
    /// Agent ID that handles this conversation.
    pub agent_id: String,
    /// Agent spawn mode.
    pub mode: BindingMode,
    /// When this binding was resolved.
    pub resolved_at: Instant,
    /// Whether this was created from an explicit rule.
    pub explicit: bool,
}

/// Agent binding mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingMode {
    /// One-shot: agent processes one message then terminates.
    OneShot,
    /// Persistent: agent stays alive for the session lifetime.
    Persistent,
    /// Ephemeral: agent created per-message, no state kept.
    Ephemeral,
}

impl std::fmt::Display for BindingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingMode::OneShot => write!(f, "oneshot"),
            BindingMode::Persistent => write!(f, "persistent"),
            BindingMode::Ephemeral => write!(f, "ephemeral"),
        }
    }
}

impl ResolvedBinding {
    /// Create a new resolved binding.
    pub fn new(
        thread_id: impl Into<String>,
        agent_id: impl Into<String>,
        mode: BindingMode,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            agent_id: agent_id.into(),
            mode,
            resolved_at: Instant::now(),
            explicit: false,
        }
    }

    /// Mark as explicitly created.
    pub fn explicit(mut self) -> Self {
        self.explicit = true;
        self
    }
}

/// Cached entry with TTL.
struct CacheEntry {
    binding: ResolvedBinding,
    inserted_at: Instant,
}

/// LRU cache for evaluated bindings.
pub struct BindingCache {
    entries: RwLock<HashMap<String, CacheEntry>>,
    ttl: Duration,
    max_size: usize,
}

impl Default for BindingCache {
    fn default() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(3600), // 1 hour default
            max_size: 10_000,
        }
    }
}

impl BindingCache {
    /// Create a new binding cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set TTL.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Set max size.
    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size = max_size;
        self
    }

    /// Get a cached binding if it exists and is not expired.
    pub async fn get(&self, key: &str) -> Option<ResolvedBinding> {
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(key) {
            if entry.inserted_at.elapsed() < self.ttl {
                return Some(entry.binding.clone());
            }
        }
        None
    }

    /// Insert a binding into the cache.
    pub async fn insert(&self, key: String, binding: ResolvedBinding) {
        let mut entries = self.entries.write().await;
        // Simple eviction: if at capacity, clear oldest half
        if entries.len() >= self.max_size {
            let keys_to_remove: Vec<String> = entries
                .iter()
                .map(|(k, v)| (k.clone(), v.inserted_at))
                .collect::<Vec<_>>()
                .into_iter()
                .filter(|(_, t)| t.elapsed() > Duration::from_secs(300))
                .map(|(k, _)| k)
                .take(self.max_size / 2)
                .collect();

            if keys_to_remove.is_empty() {
                // Nothing old enough — force-evict the oldest half to
                // prevent unbounded growth beyond max_size.
                let mut age_entries: Vec<(String, Instant)> = entries
                    .iter()
                    .map(|(k, v)| (k.clone(), v.inserted_at))
                    .collect();
                age_entries.sort_by_key(|(_, t)| *t);
                for (k, _) in age_entries.into_iter().take(self.max_size / 2) {
                    entries.remove(&k);
                }
            } else {
                for k in keys_to_remove {
                    entries.remove(&k);
                }
            }
        }
        entries.insert(
            key,
            CacheEntry {
                binding,
                inserted_at: Instant::now(),
            },
        );
    }

    /// Invalidate a cached binding.
    pub async fn invalidate(&self, key: &str) {
        let mut entries = self.entries.write().await;
        entries.remove(key);
    }

    /// Clear all entries.
    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
    }

    /// Get cache stats.
    pub async fn stats(&self) -> (usize, usize) {
        let entries = self.entries.read().await;
        let total = entries.len();
        let expired = entries
            .values()
            .filter(|e| e.inserted_at.elapsed() > self.ttl)
            .count();
        (total, expired)
    }
}

/// A routing rule that maps dimensions to an agent.
#[derive(Debug, Clone)]
pub struct RouteRule {
    /// Human-readable name.
    pub name: String,
    /// Peer pattern (exact, prefix, or * for any).
    pub peer_pattern: String,
    /// Guild pattern (optional).
    pub guild_pattern: Option<String>,
    /// Channel pattern.
    pub channel_pattern: String,
    /// Scope restriction.
    pub scope: Option<ConversationScope>,
    /// Role restriction.
    pub role_pattern: Option<String>,
    /// Target agent ID.
    pub agent_id: String,
    /// Binding mode.
    pub mode: BindingMode,
    /// Priority (higher wins).
    pub priority: i32,
}

impl RouteRule {
    /// Check if this rule matches a route resolution.
    pub fn matches(&self, resolution: &RouteResolution) -> bool {
        // Peer match
        if !Self::pattern_matches(&self.peer_pattern, &resolution.peer) {
            return false;
        }
        // Guild match
        if let Some(ref guild_pat) = self.guild_pattern {
            let guild = resolution.guild.as_deref().unwrap_or("");
            if !Self::pattern_matches(guild_pat, guild) {
                return false;
            }
        }
        // Channel match
        if !Self::pattern_matches(&self.channel_pattern, &resolution.channel) {
            return false;
        }
        // Scope match
        if let Some(scope) = self.scope {
            if scope != resolution.scope {
                return false;
            }
        }
        // Role match
        if let Some(ref role_pat) = self.role_pattern {
            let role = resolution.role.as_deref().unwrap_or("");
            if !Self::pattern_matches(role_pat, role) {
                return false;
            }
        }
        true
    }

    /// Simple glob matching: * matches anything, exact otherwise.
    fn pattern_matches(pattern: &str, text: &str) -> bool {
        if pattern == "*" || pattern == text {
            return true;
        }
        // Prefix match: pattern ends with *
        if let Some(prefix) = pattern.strip_suffix('*') {
            return text.starts_with(prefix);
        }
        false
    }
}

/// route resolver.
pub struct RouteResolver {
    /// Cached bindings.
    cache: Arc<BindingCache>,
    /// Routing rules (sorted by priority descending).
    rules: RwLock<Vec<RouteRule>>,
    /// Default agent ID (runtime-switchable via `set_default_agent`).
    default_agent_id: RwLock<String>,
    /// Default binding mode.
    default_mode: BindingMode,
    /// Session-scoped overrides (session_key -> ResolvedBinding).
    session_overrides: RwLock<HashMap<String, ResolvedBinding>>,
}

impl RouteResolver {
    /// Create a new route resolver.
    pub fn new(default_agent_id: impl Into<String>) -> Self {
        Self {
            cache: Arc::new(BindingCache::new()),
            rules: RwLock::new(Vec::new()),
            default_agent_id: RwLock::new(default_agent_id.into()),
            default_mode: BindingMode::Persistent,
            session_overrides: RwLock::new(HashMap::new()),
        }
    }

    /// Add a routing rule.
    pub async fn add_rule(&self, rule: RouteRule) {
        let mut rules = self.rules.write().await;
        rules.push(rule);
        // Sort by priority descending
        rules.sort_by_key(|b| std::cmp::Reverse(b.priority));
        debug!("Added route rule, total rules: {}", rules.len());
    }

    /// Remove a rule by name.
    pub async fn remove_rule(&self, name: &str) -> bool {
        let mut rules = self.rules.write().await;
        let before = rules.len();
        rules.retain(|r| r.name != name);
        let removed = before > rules.len();
        if removed {
            self.cache.clear().await;
        }
        removed
    }

    /// Resolve a route to a binding.
    pub async fn resolve(&self, resolution: &RouteResolution) -> ResolvedBinding {
        let cache_key = resolution.cache_key();

        // 1. Check cache
        if let Some(binding) = self.cache.get(&cache_key).await {
            debug!("Cache hit for route: {}", cache_key);
            return binding;
        }

        // 2. Check session override
        let session_key = resolution.session_key();
        {
            let overrides = self.session_overrides.read().await;
            if let Some(binding) = overrides.get(&session_key) {
                debug!("Session override for {}: agent={}", session_key, binding.agent_id);
                let binding = binding.clone();
                drop(overrides);
                self.cache.insert(cache_key, binding.clone()).await;
                return binding;
            }
        }

        // 3. Match rules (highest priority first)
        let rules = self.rules.read().await;
        let matched_rule = rules.iter().find(|r| r.matches(resolution)).cloned();
        drop(rules);

        if let Some(rule) = matched_rule {
            let binding = ResolvedBinding::new(
                format!("thread-{}", uuid::Uuid::new_v4()),
                &rule.agent_id,
                rule.mode,
            )
            .explicit();
            info!(
                "Rule '{}' matched for peer={} channel={}, agent={}",
                rule.name, resolution.peer, resolution.channel, rule.agent_id
            );
            self.cache.insert(cache_key, binding.clone()).await;
            return binding;
        }

        // 4. Default binding
        debug!(
            "No rule matched for peer={} channel={}, using default",
            resolution.peer, resolution.channel
        );
        let default_agent_id = self.default_agent_id.read().await;
        let binding = ResolvedBinding::new(
            format!("thread-{}", uuid::Uuid::new_v4()),
            default_agent_id.as_str(),
            self.default_mode,
        );
        self.cache.insert(cache_key, binding.clone()).await;
        binding
    }

    /// Switch the default agent (runtime, used by `agents.default`).
    pub async fn set_default_agent(&self, agent_id: impl Into<String>) {
        *self.default_agent_id.write().await = agent_id.into();
        // Invalidate cached bindings so the new default takes effect
        // immediately for conversations that had no explicit routing.
        self.cache.clear().await;
    }

    /// The currently configured default agent.
    pub async fn default_agent(&self) -> String {
        self.default_agent_id.read().await.clone()
    }

    /// Set a session-scoped override.
    pub async fn set_session_override(
        &self,
        session_key: impl Into<String>,
        binding: ResolvedBinding,
    ) {
        let mut overrides = self.session_overrides.write().await;
        overrides.insert(session_key.into(), binding);
    }

    /// Clear a session override and invalidate the cache.
    pub async fn clear_session_override(&self, session_key: &str) {
        let mut overrides = self.session_overrides.write().await;
        overrides.remove(session_key);
        drop(overrides);
        // Clear cache to ensure stale bindings are not returned
        self.cache.clear().await;
    }

    /// List all active rules.
    pub async fn list_rules(&self) -> Vec<RouteRule> {
        let rules = self.rules.read().await;
        rules.clone()
    }

    /// Get cache stats.
    pub async fn cache_stats(&self) -> (usize, usize) {
        self.cache.stats().await
    }

    /// Clear all caches.
    pub async fn clear_cache(&self) {
        self.cache.clear().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_route() {
        let resolver = RouteResolver::new("default");
        let resolution = RouteResolution::new("user1", "telegram", ConversationScope::Dm);
        let binding = resolver.resolve(&resolution).await;
        assert_eq!(binding.agent_id, "default");
        assert_eq!(binding.mode, BindingMode::Persistent);
    }

    #[tokio::test]
    async fn test_rule_matching() {
        let resolver = RouteResolver::new("default");

        resolver
            .add_rule(RouteRule {
                name: "admin_dm".to_string(),
                peer_pattern: "admin_*".to_string(),
                guild_pattern: None,
                channel_pattern: "*".to_string(),
                scope: Some(ConversationScope::Dm),
                role_pattern: None,
                agent_id: "admin_agent".to_string(),
                mode: BindingMode::Persistent,
                priority: 10,
            })
            .await;

        resolver
            .add_rule(RouteRule {
                name: "dev_channel".to_string(),
                peer_pattern: "*".to_string(),
                guild_pattern: None,
                channel_pattern: "dev_*".to_string(),
                scope: Some(ConversationScope::Channel),
                role_pattern: None,
                agent_id: "coder".to_string(),
                mode: BindingMode::Persistent,
                priority: 5,
            })
            .await;

        // Admin DM should match
        let admin_dm = RouteResolution::new("admin_alice", "telegram", ConversationScope::Dm);
        let binding = resolver.resolve(&admin_dm).await;
        assert_eq!(binding.agent_id, "admin_agent");

        // Regular user DM should not match, use default
        let user_dm = RouteResolution::new("user_bob", "telegram", ConversationScope::Dm);
        let binding = resolver.resolve(&user_dm).await;
        assert_eq!(binding.agent_id, "default");

        // Dev channel should match
        let dev_ch = RouteResolution::new("user_bob", "dev_team", ConversationScope::Channel);
        let binding = resolver.resolve(&dev_ch).await;
        assert_eq!(binding.agent_id, "coder");
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let resolver = RouteResolver::new("default");

        resolver
            .add_rule(RouteRule {
                name: "low".to_string(),
                peer_pattern: "*".to_string(),
                guild_pattern: None,
                channel_pattern: "*".to_string(),
                scope: None,
                role_pattern: None,
                agent_id: "low_agent".to_string(),
                mode: BindingMode::Persistent,
                priority: 1,
            })
            .await;

        resolver
            .add_rule(RouteRule {
                name: "high".to_string(),
                peer_pattern: "*".to_string(),
                guild_pattern: None,
                channel_pattern: "*".to_string(),
                scope: None,
                role_pattern: None,
                agent_id: "high_agent".to_string(),
                mode: BindingMode::Persistent,
                priority: 100,
            })
            .await;

        let resolution = RouteResolution::new("user1", "ch1", ConversationScope::Dm);
        let binding = resolver.resolve(&resolution).await;
        assert_eq!(binding.agent_id, "high_agent");
    }

    #[tokio::test]
    async fn test_session_override() {
        let resolver = RouteResolver::new("default");
        let resolution = RouteResolution::new("user1", "ch1", ConversationScope::Dm);
        let session_key = resolution.session_key();

        // Set override
        resolver
            .set_session_override(
                session_key.clone(),
                ResolvedBinding::new("thread-1", "special_agent", BindingMode::Persistent),
            )
            .await;

        let binding = resolver.resolve(&resolution).await;
        assert_eq!(binding.agent_id, "special_agent");

        // Clear override
        resolver.clear_session_override(&session_key).await;
        let binding = resolver.resolve(&resolution).await;
        assert_eq!(binding.agent_id, "default");
    }

    #[tokio::test]
    async fn test_caching() {
        let resolver = RouteResolver::new("default");
        let resolution = RouteResolution::new("user1", "ch1", ConversationScope::Dm);

        // First resolve
        let binding1 = resolver.resolve(&resolution).await;
        // Second resolve should hit cache
        let binding2 = resolver.resolve(&resolution).await;

        assert_eq!(binding1.agent_id, binding2.agent_id);
        assert_eq!(binding1.thread_id, binding2.thread_id);
    }

    #[test]
    fn test_pattern_matching() {
        assert!(RouteRule::pattern_matches("*", "anything"));
        assert!(RouteRule::pattern_matches("admin_*", "admin_alice"));
        assert!(!RouteRule::pattern_matches("admin_*", "user_alice"));
        assert!(RouteRule::pattern_matches("exact", "exact"));
        assert!(!RouteRule::pattern_matches("exact", "other"));
    }
}
