//! Account snapshot system for Syscity channels
//!
//! Provides channel account state snapshots with diagnostic display tones:
//! - `default` — Normal operational state
//! - `muted` — Channel is connected but inactive/suppressed
//! - `success` — Channel is healthy and operational
//! - `warn` — Channel has non-critical issues
//! - `error` — Channel has critical issues

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Diagnostic display tone for an account snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayTone {
    /// Normal operational state.
    Default,
    /// Channel connected but inactive/suppressed.
    Muted,
    /// Channel healthy and operational.
    Success,
    /// Non-critical issues present.
    Warn,
    /// Critical issues present.
    Error,
}

impl DisplayTone {
    /// Return the CSS/terminal color class for this tone.
    pub fn color_class(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Muted => "muted",
            Self::Success => "success",
            Self::Warn => "warning",
            Self::Error => "error",
        }
    }

    /// Return a human-readable label for this tone.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Default => "Normal",
            Self::Muted => "Muted",
            Self::Success => "Operational",
            Self::Warn => "Warning",
            Self::Error => "Error",
        }
    }

    /// Return the emoji representation (for display purposes).
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Default => "⚪",
            Self::Muted => "🔇",
            Self::Success => "✅",
            Self::Warn => "⚠️",
            Self::Error => "❌",
        }
    }
}

/// A snapshot of a channel account's state at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountSnapshot {
    /// Channel name (e.g., "telegram", "discord").
    pub channel_name: String,
    /// Account/bot identifier.
    pub account_id: Option<String>,
    /// Current status description.
    pub status: String,
    /// Display tone for diagnostic rendering.
    pub tone: DisplayTone,
    /// When the snapshot was taken.
    pub timestamp: DateTime<Utc>,
    /// Whether the channel is currently connected.
    pub connected: bool,
    /// Number of active conversations.
    pub active_conversations: usize,
    /// Message count since last reset.
    pub message_count: u64,
    /// Error count since last reset.
    pub error_count: u64,
    /// Last error message (if any).
    pub last_error: Option<String>,
    /// Health status string (e.g., "healthy", "degraded", "unhealthy").
    pub health_status: String,
    /// Additional channel-specific metrics.
    pub metrics: HashMap<String, serde_json::Value>,
}

impl AccountSnapshot {
    /// Create a new account snapshot.
    pub fn new(channel_name: impl Into<String>) -> Self {
        Self {
            channel_name: channel_name.into(),
            account_id: None,
            status: "initializing".to_string(),
            tone: DisplayTone::Default,
            timestamp: Utc::now(),
            connected: false,
            active_conversations: 0,
            message_count: 0,
            error_count: 0,
            last_error: None,
            health_status: "unknown".to_string(),
            metrics: HashMap::new(),
        }
    }

    /// Set the account ID.
    pub fn with_account(mut self, id: impl Into<String>) -> Self {
        self.account_id = Some(id.into());
        self
    }

    /// Set the status and tone.
    pub fn with_status(mut self, status: impl Into<String>, tone: DisplayTone) -> Self {
        self.status = status.into();
        self.tone = tone;
        self
    }

    /// Mark as connected.
    pub fn connected(mut self) -> Self {
        self.connected = true;
        self
    }

    /// Set the health status.
    pub fn with_health(mut self, health: impl Into<String>) -> Self {
        self.health_status = health.into();
        self
    }

    /// Record an error.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error_count += 1;
        self.last_error = Some(error.into());
        self.tone = DisplayTone::Error;
        self
    }

    /// Set active conversation count.
    pub fn with_conversations(mut self, count: usize) -> Self {
        self.active_conversations = count;
        self
    }

    /// Increment message count.
    pub fn increment_messages(&mut self) {
        self.message_count += 1;
    }

    /// Add a metric key-value pair.
    pub fn with_metric(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.metrics.insert(key.into(), value.into());
        self
    }
}

/// Store for channel account snapshots.
#[derive(Debug, Clone)]
pub struct AccountSnapshotStore {
    /// Per-channel, per-account snapshots.
    snapshots: Arc<RwLock<HashMap<String, HashMap<String, AccountSnapshot>>>>,
}

impl AccountSnapshotStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            snapshots: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store a snapshot.
    pub async fn store(&self, snapshot: AccountSnapshot) {
        let channel = snapshot.channel_name.clone();
        let account = snapshot
            .account_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let mut snapshots = self.snapshots.write().await;
        snapshots
            .entry(channel)
            .or_insert_with(HashMap::new)
            .insert(account, snapshot);
    }

    /// Get a snapshot for a specific channel and account.
    pub async fn get(
        &self,
        channel_name: &str,
        account_id: Option<&str>,
    ) -> Option<AccountSnapshot> {
        let account = account_id.unwrap_or("default");
        let snapshots = self.snapshots.read().await;
        snapshots
            .get(channel_name)
            .and_then(|ch| ch.get(account))
            .cloned()
    }

    /// Get all snapshots for a channel.
    pub async fn get_channel_snapshots(&self, channel_name: &str) -> Vec<AccountSnapshot> {
        let snapshots = self.snapshots.read().await;
        snapshots
            .get(channel_name)
            .map(|ch| ch.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get all snapshots across all channels.
    pub async fn get_all(&self) -> Vec<AccountSnapshot> {
        let snapshots = self.snapshots.read().await;
        snapshots
            .values()
            .flat_map(|ch| ch.values().cloned())
            .collect()
    }

    /// Get the worst tone across all channels.
    pub async fn worst_tone(&self) -> DisplayTone {
        let snapshots = self.snapshots.read().await;
        snapshots
            .values()
            .flat_map(|ch| ch.values())
            .map(|s| s.tone)
            .max_by_key(|t| match t {
                DisplayTone::Error => 4,
                DisplayTone::Warn => 3,
                DisplayTone::Muted => 2,
                DisplayTone::Success => 1,
                DisplayTone::Default => 0,
            })
            .unwrap_or(DisplayTone::Default)
    }

    /// Remove a snapshot for a specific channel and account.
    pub async fn remove(&self, channel_name: &str, account_id: Option<&str>) {
        let account = account_id.unwrap_or("default");
        let mut snapshots = self.snapshots.write().await;
        if let Some(ch) = snapshots.get_mut(channel_name) {
            ch.remove(account);
        }
    }

    /// Clear all snapshots.
    pub async fn clear(&self) {
        let mut snapshots = self.snapshots.write().await;
        snapshots.clear();
    }

    /// Generate a diagnostic summary of all channel state.
    pub async fn diagnostic_summary(&self) -> String {
        let all = self.get_all().await;
        if all.is_empty() {
            return "No channel snapshots available.".to_string();
        }

        let mut lines = Vec::new();
        for snap in &all {
            let tone_marker = snap.tone.emoji();
            let account = snap.account_id.as_deref().unwrap_or("default");
            lines.push(format!(
                "{} {} [{}] {} — {} msgs, {} errs, status: {}",
                tone_marker,
                snap.channel_name,
                account,
                snap.status,
                snap.message_count,
                snap.error_count,
                snap.health_status,
            ));
            if let Some(ref err) = snap.last_error {
                lines.push(format!("   Last error: {}", err));
            }
        }

        lines.join("\n")
    }
}

impl Default for AccountSnapshotStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a healthy snapshot for a channel.
pub fn healthy_snapshot(
    channel_name: impl Into<String>,
    account_id: Option<String>,
) -> AccountSnapshot {
    AccountSnapshot::new(channel_name)
        .with_account(account_id.unwrap_or_default())
        .with_status("operational", DisplayTone::Success)
        .connected()
        .with_health("healthy")
}

/// Build an error snapshot for a channel.
pub fn error_snapshot(
    channel_name: impl Into<String>,
    account_id: Option<String>,
    error: impl Into<String>,
) -> AccountSnapshot {
    AccountSnapshot::new(channel_name)
        .with_account(account_id.unwrap_or_default())
        .with_status("error", DisplayTone::Error)
        .with_health("unhealthy")
        .with_error(error)
}

/// Build a warning snapshot for a channel.
pub fn warning_snapshot(
    channel_name: impl Into<String>,
    account_id: Option<String>,
    warning: impl Into<String>,
) -> AccountSnapshot {
    AccountSnapshot::new(channel_name)
        .with_account(account_id.unwrap_or_default())
        .with_status(warning, DisplayTone::Warn)
        .connected()
        .with_health("degraded")
}

/// Build a muted snapshot for a channel.
pub fn muted_snapshot(
    channel_name: impl Into<String>,
    account_id: Option<String>,
) -> AccountSnapshot {
    AccountSnapshot::new(channel_name)
        .with_account(account_id.unwrap_or_default())
        .with_status("suppressed", DisplayTone::Muted)
        .with_health("muted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_account_snapshot_new() {
        let snap = AccountSnapshot::new("telegram").with_account("bot123");
        assert_eq!(snap.channel_name, "telegram");
        assert_eq!(snap.account_id, Some("bot123".to_string()));
        assert!(!snap.connected);
        assert_eq!(snap.tone, DisplayTone::Default);
    }

    #[tokio::test]
    async fn test_healthy_snapshot() {
        let snap = healthy_snapshot("discord", Some("bot456".to_string()));
        assert_eq!(snap.tone, DisplayTone::Success);
        assert!(snap.connected);
        assert_eq!(snap.health_status, "healthy");
    }

    #[tokio::test]
    async fn test_error_snapshot() {
        let snap = error_snapshot("telegram", None, "Connection refused");
        assert_eq!(snap.tone, DisplayTone::Error);
        assert_eq!(snap.last_error, Some("Connection refused".to_string()));
        assert!(!snap.connected);
    }

    #[tokio::test]
    async fn test_store_and_retrieve() {
        let store = AccountSnapshotStore::new();
        let snap = healthy_snapshot("telegram", Some("bot1".to_string()));
        store.store(snap).await;

        let retrieved = store.get("telegram", Some("bot1")).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().health_status, "healthy");
    }

    #[tokio::test]
    async fn test_store_overwrites() {
        let store = AccountSnapshotStore::new();
        store
            .store(healthy_snapshot("telegram", Some("bot1".to_string())))
            .await;
        store
            .store(error_snapshot("telegram", Some("bot1".to_string()), "timeout"))
            .await;

        let retrieved = store.get("telegram", Some("bot1")).await.unwrap();
        assert_eq!(retrieved.tone, DisplayTone::Error);
    }

    #[tokio::test]
    async fn test_get_channel_snapshots() {
        let store = AccountSnapshotStore::new();
        store
            .store(healthy_snapshot("telegram", Some("bot1".to_string())))
            .await;
        store
            .store(error_snapshot("telegram", Some("bot2".to_string()), "err"))
            .await;
        store
            .store(healthy_snapshot("discord", Some("bot3".to_string())))
            .await;

        let tg = store.get_channel_snapshots("telegram").await;
        assert_eq!(tg.len(), 2);

        let all = store.get_all().await;
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_worst_tone() {
        let store = AccountSnapshotStore::new();
        store
            .store(healthy_snapshot("telegram", Some("bot1".to_string())))
            .await;
        assert_eq!(store.worst_tone().await, DisplayTone::Success);

        store
            .store(error_snapshot("discord", Some("bot2".to_string()), "err"))
            .await;
        assert_eq!(store.worst_tone().await, DisplayTone::Error);
    }

    #[tokio::test]
    async fn test_remove() {
        let store = AccountSnapshotStore::new();
        store
            .store(healthy_snapshot("telegram", Some("bot1".to_string())))
            .await;
        store.remove("telegram", Some("bot1")).await;
        assert!(store.get("telegram", Some("bot1")).await.is_none());
    }

    #[tokio::test]
    async fn test_diagnostic_summary() {
        let store = AccountSnapshotStore::new();
        let summary = store.diagnostic_summary().await;
        assert_eq!(summary, "No channel snapshots available.");

        store
            .store(healthy_snapshot("telegram", Some("bot1".to_string())))
            .await;
        let summary = store.diagnostic_summary().await;
        assert!(summary.contains("telegram"));
        assert!(summary.contains("✅"));
    }

    #[test]
    fn test_display_tone_ordering() {
        assert!(DisplayTone::Error.color_class() == "error");
        assert!(DisplayTone::Success.label() == "Operational");
        assert!(DisplayTone::Warn.emoji() == "⚠️");
    }
}
