//! Mention Gating for Syscity
//!
//! Controls which mentions trigger agent responses.
//! ts`.
//!
//! # Policies
//!
//! | Policy | Behavior |
//! |----------|----------------------------------------------------|
//! | `Allow` | Allow all mentions (default, no restriction) |
//! | `Block` | Block all mentions |
//! | `Allowlist` | Only respond to mentions on the allowlist |
//! | `Blocklist` | Respond to all mentions except those on blocklist |
//!
//! # Example
//!
//! ```rust
//! use syscity::security::mention_gate::{MentionGate, MentionPolicy};
//!
//! # async fn example() {
//! let gate = MentionGate::new(MentionPolicy::Allowlist);
//!
//! // Add allowed mentions
//! gate.add_allowlist("telegram", "@alice").await;
//! gate.add_allowlist("telegram", "@bob").await;
//!
//! // Check if a mention should trigger a response
//! assert!(gate.check("telegram", "@alice").await);
//! assert!(!gate.check("telegram", "@charlie").await);
//! # }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Mention gating configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionGatingConfig {
 /// Enable mention gating.
    #[serde(default = "default_true")]
    pub enabled: bool,
 /// Default policy when enabled.
    #[serde(default)]
    pub policy: MentionPolicy,
 /// Global allowlist (applies to all channels).
    #[serde(default)]
    pub allowlist: Vec<String>,
 /// Global blocklist (applies to all channels).
    #[serde(default)]
    pub blocklist: Vec<String>,
}

impl Default for MentionGatingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            policy: MentionPolicy::Allow,
            allowlist: Vec::new(),
            blocklist: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

// ── Policy ────────────────────────────────────────────────────────────────────

/// Mention gating policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum MentionPolicy {
 /// Allow all mentions (no restriction).
    #[default]
    Allow,
 /// Block all mentions.
    Block,
 /// Only respond to mentions on the allowlist.
    Allowlist,
 /// Respond to all mentions except those on the blocklist.
    Blocklist,
}

impl std::fmt::Display for MentionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MentionPolicy::Allow => write!(f, "allow"),
            MentionPolicy::Block => write!(f, "block"),
            MentionPolicy::Allowlist => write!(f, "allowlist"),
            MentionPolicy::Blocklist => write!(f, "blocklist"),
        }
    }
}

// ── Per-channel lists ─────────────────────────────────────────────────────────

/// Allowlist / blocklist entries for a single channel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelMentions {
 /// Mention patterns on the allowlist.
    pub allowlist: Vec<String>,
 /// Mention patterns on the blocklist.
    pub blocklist: Vec<String>,
}

// ── Mention Gate ──────────────────────────────────────────────────────────────

/// Gate that decides whether a mention should trigger an agent response.
#[derive(Debug, Clone)]
pub struct MentionGate {
 /// Global policy.
    policy: Arc<RwLock<MentionPolicy>>,
 /// Per-channel mention lists.
    channels: Arc<RwLock<HashMap<String, ChannelMentions>>>,
}

impl MentionGate {
 /// Create a new mention gate with the given policy.
    pub fn new(policy: MentionPolicy) -> Self {
        Self {
            policy: Arc::new(RwLock::new(policy)),
            channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

 /// Create with default `Allow` policy.
    pub fn default_allow() -> Self {
        Self::new(MentionPolicy::Allow)
    }

 /// Get the current global policy.
    pub async fn policy(&self) -> MentionPolicy {
        *self.policy.read().await
    }

 /// Set the global policy.
    pub async fn set_policy(&self, policy: MentionPolicy) {
        let mut p = self.policy.write().await;
        *p = policy;
        info!("Mention gate policy set to: {}", policy);
    }

 /// Check whether a mention should trigger a response.
 ///
 /// Returns `true` if the agent should respond to this mention.
    pub async fn check(&self, channel: &str, mention: &str) -> bool {
        let policy = *self.policy.read().await;

        match policy {
            MentionPolicy::Allow => {
                debug!("Mention gate (allow): {} on {}", mention, channel);
                true
            }
            MentionPolicy::Block => {
                debug!("Mention gate (block): {} on {}", mention, channel);
                false
            }
            MentionPolicy::Allowlist => {
                let channels = self.channels.read().await;
                let allowed = channels
                    .get(channel)
                    .map(|c| c.allowlist.iter().any(|p| pattern_matches(p, mention)))
                    .unwrap_or(false);
                debug!(
                    "Mention gate (allowlist): {} on {} -> allowed={}",
                    mention, channel, allowed
                );
                allowed
            }
            MentionPolicy::Blocklist => {
                let channels = self.channels.read().await;
                let blocked = channels
                    .get(channel)
                    .map(|c| c.blocklist.iter().any(|p| pattern_matches(p, mention)))
                    .unwrap_or(false);
                debug!(
                    "Mention gate (blocklist): {} on {} -> blocked={}",
                    mention, channel, blocked
                );
                !blocked
            }
        }
    }

 /// Add a mention pattern to the allowlist for a channel.
    pub async fn add_allowlist(&self, channel: impl Into<String>, pattern: impl Into<String>) {
        let mut channels = self.channels.write().await;
        let channel = channel.into();
        let entry = channels.entry(channel.clone()).or_default();
        let pattern = pattern.into();
        if !entry.allowlist.contains(&pattern) {
            entry.allowlist.push(pattern.clone());
            info!("Added '{}' to allowlist for channel '{}'", pattern, channel);
        }
    }

 /// Remove a mention pattern from the allowlist for a channel.
    pub async fn remove_allowlist(&self, channel: &str, pattern: &str) -> bool {
        let mut channels = self.channels.write().await;
        if let Some(entry) = channels.get_mut(channel) {
            let before = entry.allowlist.len();
            entry.allowlist.retain(|p| p != pattern);
            let removed = entry.allowlist.len() < before;
            if removed {
                info!("Removed '{}' from allowlist for channel '{}'", pattern, channel);
            }
            removed
        } else {
            false
        }
    }

 /// Add a mention pattern to the blocklist for a channel.
    pub async fn add_blocklist(&self, channel: impl Into<String>, pattern: impl Into<String>) {
        let mut channels = self.channels.write().await;
        let channel = channel.into();
        let entry = channels.entry(channel.clone()).or_default();
        let pattern = pattern.into();
        if !entry.blocklist.contains(&pattern) {
            entry.blocklist.push(pattern.clone());
            info!("Added '{}' to blocklist for channel '{}'", pattern, channel);
        }
    }

 /// Remove a mention pattern from the blocklist for a channel.
    pub async fn remove_blocklist(&self, channel: &str, pattern: &str) -> bool {
        let mut channels = self.channels.write().await;
        if let Some(entry) = channels.get_mut(channel) {
            let before = entry.blocklist.len();
            entry.blocklist.retain(|p| p != pattern);
            let removed = entry.blocklist.len() < before;
            if removed {
                info!("Removed '{}' from blocklist for channel '{}'", pattern, channel);
            }
            removed
        } else {
            false
        }
    }

 /// Get all allowlist entries for a channel.
    pub async fn list_allowlist(&self, channel: &str) -> Vec<String> {
        let channels = self.channels.read().await;
        channels
            .get(channel)
            .map(|c| c.allowlist.clone())
            .unwrap_or_default()
    }

 /// Get all blocklist entries for a channel.
    pub async fn list_blocklist(&self, channel: &str) -> Vec<String> {
        let channels = self.channels.read().await;
        channels
            .get(channel)
            .map(|c| c.blocklist.clone())
            .unwrap_or_default()
    }

 /// Get all channels with configured mention lists.
    pub async fn list_channels(&self) -> Vec<String> {
        let channels = self.channels.read().await;
        channels.keys().cloned().collect()
    }

 /// Clear all mention lists for a channel.
    pub async fn clear_channel(&self, channel: &str) {
        let mut channels = self.channels.write().await;
        channels.remove(channel);
        info!("Cleared mention lists for channel '{}'", channel);
    }

 /// Export configuration as JSON.
    pub async fn export_json(&self) -> Result<String, serde_json::Error> {
        let channels = self.channels.read().await;
        serde_json::to_string_pretty(&*channels)
    }
}

impl Default for MentionGate {
    fn default() -> Self {
        Self::default_allow()
    }
}

// ── Pattern matching ──────────────────────────────────────────────────────────

/// Check if a mention matches a pattern.
///
/// Supports:
/// - Exact string match
/// - `*` wildcard at start or end (e.g. `*bot`, `spam*`)
/// - `*` as full wildcard matching everything
fn pattern_matches(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern.starts_with('*') && pattern.ends_with('*') {
        let inner = &pattern[1..pattern.len() - 1];
        return text.contains(inner);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return text.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return text.starts_with(prefix);
    }
    pattern == text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_allow_policy() {
        let gate = MentionGate::new(MentionPolicy::Allow);
        assert!(gate.check("telegram", "@alice").await);
        assert!(gate.check("discord", "@bob").await);
    }

    #[tokio::test]
    async fn test_block_policy() {
        let gate = MentionGate::new(MentionPolicy::Block);
        assert!(!gate.check("telegram", "@alice").await);
        assert!(!gate.check("discord", "@bob").await);
    }

    #[tokio::test]
    async fn test_allowlist() {
        let gate = MentionGate::new(MentionPolicy::Allowlist);
        gate.add_allowlist("telegram", "@alice").await;
        gate.add_allowlist("telegram", "@bob").await;

        assert!(gate.check("telegram", "@alice").await);
        assert!(gate.check("telegram", "@bob").await);
        assert!(!gate.check("telegram", "@charlie").await);
        assert!(!gate.check("discord", "@alice").await);
    }

    #[tokio::test]
    async fn test_blocklist() {
        let gate = MentionGate::new(MentionPolicy::Blocklist);
        gate.add_blocklist("telegram", "@spam").await;
        gate.add_blocklist("telegram", "@bot*").await;

        assert!(gate.check("telegram", "@alice").await);
        assert!(!gate.check("telegram", "@spam").await);
        assert!(!gate.check("telegram", "@bot123").await);
        assert!(gate.check("discord", "@spam").await);
    }

    #[test]
    fn test_pattern_matches() {
        assert!(pattern_matches("*", "anything"));
        assert!(pattern_matches("@alice", "@alice"));
        assert!(pattern_matches("spam*", "spammer"));
        assert!(pattern_matches("*bot", "mybot"));
        assert!(pattern_matches("*test*", "this is a test message"));
        assert!(!pattern_matches("@alice", "@bob"));
        assert!(!pattern_matches("spam*", "ham"));
    }

    #[tokio::test]
    async fn test_default_allow() {
        let gate = MentionGate::default_allow();
        assert_eq!(gate.policy().await, MentionPolicy::Allow);
        assert!(gate.check("any", "@mention").await);
    }

    #[tokio::test]
    async fn test_set_policy() {
        let gate = MentionGate::new(MentionPolicy::Allow);
        gate.set_policy(MentionPolicy::Block).await;
        assert_eq!(gate.policy().await, MentionPolicy::Block);
        assert!(!gate.check("telegram", "@alice").await);
    }

    #[tokio::test]
    async fn test_remove_allowlist() {
        let gate = MentionGate::new(MentionPolicy::Allowlist);
        gate.add_allowlist("telegram", "@alice").await;
        assert!(gate.check("telegram", "@alice").await);

        let removed = gate.remove_allowlist("telegram", "@alice").await;
        assert!(removed);
        assert!(!gate.check("telegram", "@alice").await);

        let not_found = gate.remove_allowlist("telegram", "@bob").await;
        assert!(!not_found);
    }

    #[tokio::test]
    async fn test_remove_blocklist() {
        let gate = MentionGate::new(MentionPolicy::Blocklist);
        gate.add_blocklist("telegram", "@spam").await;
        assert!(!gate.check("telegram", "@spam").await);

        let removed = gate.remove_blocklist("telegram", "@spam").await;
        assert!(removed);
        assert!(gate.check("telegram", "@spam").await);
    }

    #[tokio::test]
    async fn test_list_allowlist() {
        let gate = MentionGate::new(MentionPolicy::Allowlist);
        gate.add_allowlist("telegram", "@alice").await;
        gate.add_allowlist("telegram", "@bob").await;

        let list = gate.list_allowlist("telegram").await;
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"@alice".to_string()));
        assert!(list.contains(&"@bob".to_string()));

        let empty = gate.list_allowlist("discord").await;
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_list_blocklist() {
        let gate = MentionGate::new(MentionPolicy::Blocklist);
        gate.add_blocklist("telegram", "@spam").await;

        let list = gate.list_blocklist("telegram").await;
        assert_eq!(list, vec!["@spam"]);

        let empty = gate.list_blocklist("discord").await;
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_list_channels() {
        let gate = MentionGate::new(MentionPolicy::Allowlist);
        gate.add_allowlist("telegram", "@alice").await;
        gate.add_blocklist("discord", "@spam").await;

        let channels = gate.list_channels().await;
        assert_eq!(channels.len(), 2);
        assert!(channels.contains(&"telegram".to_string()));
        assert!(channels.contains(&"discord".to_string()));
    }

    #[tokio::test]
    async fn test_clear_channel() {
        let gate = MentionGate::new(MentionPolicy::Allowlist);
        gate.add_allowlist("telegram", "@alice").await;
        assert!(gate.check("telegram", "@alice").await);

        gate.clear_channel("telegram").await;
        assert!(!gate.check("telegram", "@alice").await);
        assert!(gate.list_channels().await.is_empty());
    }

    #[tokio::test]
    async fn test_export_json() {
        let gate = MentionGate::new(MentionPolicy::Allowlist);
        gate.add_allowlist("telegram", "@alice").await;
        gate.add_blocklist("telegram", "@spam").await;

        let json = gate.export_json().await.unwrap();
        assert!(json.contains("telegram"));
        assert!(json.contains("@alice"));
        assert!(json.contains("@spam"));
    }

    #[test]
    fn test_mention_policy_display() {
        assert_eq!(MentionPolicy::Allow.to_string(), "allow");
        assert_eq!(MentionPolicy::Block.to_string(), "block");
        assert_eq!(MentionPolicy::Allowlist.to_string(), "allowlist");
        assert_eq!(MentionPolicy::Blocklist.to_string(), "blocklist");
    }

    #[test]
    fn test_mention_policy_default() {
        assert_eq!(MentionPolicy::default(), MentionPolicy::Allow);
    }

    #[test]
    fn test_mention_gating_config_default() {
        let config = MentionGatingConfig::default();
        assert!(config.enabled);
        assert_eq!(config.policy, MentionPolicy::Allow);
        assert!(config.allowlist.is_empty());
        assert!(config.blocklist.is_empty());
    }

    #[test]
    fn test_mention_gating_config_serde() {
        let json = r#"{"enabled": false, "policy": "blocklist", "allowlist": ["@alice"]}"#;
        let config: MentionGatingConfig = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.policy, MentionPolicy::Blocklist);
        assert_eq!(config.allowlist, vec!["@alice"]);
    }

    #[test]
    fn test_channel_mentions_default() {
        let cm = ChannelMentions::default();
        assert!(cm.allowlist.is_empty());
        assert!(cm.blocklist.is_empty());
    }

    #[test]
    fn test_mention_gate_default() {
        let gate: MentionGate = Default::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let policy = rt.block_on(gate.policy());
        assert_eq!(policy, MentionPolicy::Allow);
    }
}
