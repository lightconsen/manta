//! Auth Profile Rotation — Multi-key API key management for providers
//!
//! Provides automatic rotation of API keys when a provider returns
//! rate-limit (429) or auth (401/403) errors. Each provider can be
//! configured with multiple keys; the system rotates to the next
//! available key after a cooldown period.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::model_router::auth_profile_store::AuthProfileStore;

/// Status of an individual API key within a profile
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyStatus {
    /// Key is healthy and available
    Active,
    /// Key is temporarily on cooldown after a failure
    Cooldown,
    /// Key has been permanently disabled
    Disabled,
}

/// Individual key entry with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEntry {
    /// The API key value (masked in serialization)
    #[serde(skip)]
    pub key: String,
    /// Human-readable label (e.g., "primary", "secondary")
    pub label: String,
    /// Current status
    pub status: KeyStatus,
    /// Number of consecutive failures
    pub failure_count: u32,
    /// Total successful requests
    pub success_count: u64,
    /// Last failure timestamp
    pub last_failure: Option<DateTime<Utc>>,
    /// Cooldown expires at
    pub cooldown_until: Option<DateTime<Utc>>,
}

impl KeyEntry {
    /// Create a new key entry
    pub fn new(key: String, label: impl Into<String>) -> Self {
        Self {
            key,
            label: label.into(),
            status: KeyStatus::Active,
            failure_count: 0,
            success_count: 0,
            last_failure: None,
            cooldown_until: None,
        }
    }

    /// Check if this key is currently available for use
    pub fn is_available(&self) -> bool {
        if self.status == KeyStatus::Disabled {
            return false;
        }
        if let Some(until) = self.cooldown_until {
            return Utc::now() >= until;
        }
        true
    }

    /// Record a successful request
    pub fn record_success(&mut self) {
        self.success_count += 1;
        self.failure_count = 0;
        self.status = KeyStatus::Active;
        self.cooldown_until = None;
    }

    /// Record a failure and optionally put key on cooldown
    pub fn record_failure(&mut self, cooldown_secs: u64) {
        self.failure_count += 1;
        self.last_failure = Some(Utc::now());
        if cooldown_secs > 0 {
            self.cooldown_until =
                Some(Utc::now() + chrono::Duration::seconds(cooldown_secs as i64));
            self.status = KeyStatus::Cooldown;
        }
    }

    /// Mask the key for safe display
    pub fn masked_key(&self) -> String {
        if self.key.len() <= 8 {
            "****".to_string()
        } else {
            format!("{}****", &self.key[..4])
        }
    }
}

/// Auth profile for a single provider — manages multiple API keys
#[derive(Debug, Clone)]
pub struct AuthProfile {
    /// Provider name this profile belongs to
    pub provider_name: String,
    /// All configured keys
    keys: Vec<KeyEntry>,
    /// Currently active key index
    current_index: usize,
    /// Cooldown duration after a failure (seconds)
    #[allow(dead_code)]
    cooldown_secs: u64,
    /// Max failures before disabling a key permanently
    max_failures: u32,
}

impl AuthProfile {
    /// Create a new auth profile with a single key
    pub fn single_key(provider_name: impl Into<String>, key: String) -> Self {
        Self::with_keys(provider_name, vec![(key, "primary")], 60, 3)
    }

    /// Create a profile with multiple keys
    pub fn with_keys(
        provider_name: impl Into<String>,
        keys: Vec<(String, impl Into<String>)>,
        cooldown_secs: u64,
        max_failures: u32,
    ) -> Self {
        let entries: Vec<KeyEntry> = keys
            .into_iter()
            .enumerate()
            .map(|(i, (key, label))| {
                let label = label.into();
                KeyEntry::new(
                    key,
                    if label.is_empty() {
                        format!("key-{}", i)
                    } else {
                        label
                    },
                )
            })
            .collect();

        Self {
            provider_name: provider_name.into(),
            keys: entries,
            current_index: 0,
            cooldown_secs,
            max_failures,
        }
    }

    /// Get the currently active key
    pub fn current_key(&self) -> Option<&str> {
        self.keys.get(self.current_index).map(|k| k.key.as_str())
    }

    /// Get the current key entry (mutable)
    fn current_entry_mut(&mut self) -> Option<&mut KeyEntry> {
        self.keys.get_mut(self.current_index)
    }

    /// Rotate to the next available key. Returns the new key if found.
    ///
    /// `cooldown_secs` overrides the profile default for this rotation.
    pub fn rotate(&mut self, cooldown_secs: u64) -> Option<String> {
        // Extract scalar values before mutable borrow
        let provider_name = self.provider_name.clone();
        let max_failures = self.max_failures;

        // Mark current key as failed
        if let Some(entry) = self.current_entry_mut() {
            entry.record_failure(cooldown_secs);
            if entry.failure_count >= max_failures {
                entry.status = KeyStatus::Disabled;
                warn!(
                    "Auth key '{}' for provider '{}' permanently disabled after {} failures",
                    entry.label, provider_name, entry.failure_count
                );
            } else {
                info!(
                    "Auth key '{}' for provider '{}' put on {}s cooldown (failure {}/{})",
                    entry.label, provider_name, cooldown_secs, entry.failure_count, max_failures
                );
            }
        }

        // Find next available key
        let start = self.current_index;
        let count = self.keys.len();

        for offset in 1..=count {
            let idx = (start + offset) % count;
            if self.keys[idx].is_available() {
                self.current_index = idx;
                info!(
                    "Rotated auth profile for provider '{}' to key '{}' (index {})",
                    provider_name, self.keys[idx].label, idx
                );
                return Some(self.keys[idx].key.clone());
            }
        }

        warn!(
            "No available auth keys for provider '{}' (all on cooldown or disabled)",
            provider_name
        );
        None
    }

    /// Record a success on the current key
    pub fn record_success(&mut self) {
        if let Some(entry) = self.current_entry_mut() {
            entry.record_success();
        }
    }

    /// Get all key statuses (for API responses)
    pub fn key_statuses(&self) -> Vec<KeyStatusInfo> {
        self.keys
            .iter()
            .map(|k| KeyStatusInfo {
                label: k.label.clone(),
                masked_key: k.masked_key(),
                status: k.status,
                failure_count: k.failure_count,
                success_count: k.success_count,
                last_failure: k.last_failure,
                cooldown_until: k.cooldown_until,
                is_available: k.is_available(),
            })
            .collect()
    }

    /// Number of keys configured
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// Number of currently available keys
    pub fn available_count(&self) -> usize {
        self.keys.iter().filter(|k| k.is_available()).count()
    }

    /// Get a mutable reference to a key entry by label.
    pub fn key_entry_mut(&mut self, label: &str) -> Option<&mut KeyEntry> {
        self.keys.iter_mut().find(|k| k.label == label)
    }
}

/// Key status info for API responses
#[derive(Debug, Clone, Serialize)]
pub struct KeyStatusInfo {
    pub label: String,
    pub masked_key: String,
    pub status: KeyStatus,
    pub failure_count: u32,
    pub success_count: u64,
    pub last_failure: Option<DateTime<Utc>>,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub is_available: bool,
}

/// Profile status for API responses
#[derive(Debug, Clone, Serialize)]
pub struct ProfileStatus {
    pub provider_name: String,
    pub current_key_label: String,
    pub current_key_masked: String,
    pub total_keys: usize,
    pub available_keys: usize,
    pub keys: Vec<KeyStatusInfo>,
}

/// Auth profile configuration (TOML-serializable)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfileConfig {
    /// Multiple API keys with optional labels
    pub keys: Vec<AuthKeyConfig>,
    /// Cooldown duration in seconds after a failure (default: 60)
    #[serde(default = "default_cooldown_secs")]
    pub cooldown_secs: u64,
    /// Max failures before permanently disabling a key (default: 3)
    #[serde(default = "default_max_failures")]
    pub max_failures: u32,
}

fn default_cooldown_secs() -> u64 {
    60
}

fn default_max_failures() -> u32 {
    3
}

/// Single key configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthKeyConfig {
    /// The API key value
    pub key: String,
    /// Human-readable label
    #[serde(default)]
    pub label: String,
}

impl Default for AuthProfileConfig {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            cooldown_secs: default_cooldown_secs(),
            max_failures: default_max_failures(),
        }
    }
}

/// Manages auth profiles for all providers
#[derive(Debug, Default)]
pub struct AuthProfileManager {
    profiles: RwLock<HashMap<String, AuthProfile>>,
    store: RwLock<Option<Arc<AuthProfileStore>>>,
}

impl AuthProfileManager {
    /// Create a new auth profile manager
    pub fn new() -> Self {
        Self {
            profiles: RwLock::new(HashMap::new()),
            store: RwLock::new(None),
        }
    }

    /// Attach a persistent store for saving/loading key state.
    pub async fn set_store(&self, store: Arc<AuthProfileStore>) {
        let mut s = self.store.write().await;
        *s = Some(store);
    }

    /// Register a profile for a provider
    pub async fn register_profile(&self, provider_name: &str, profile: AuthProfile) {
        let mut profiles = self.profiles.write().await;
        profiles.insert(provider_name.to_string(), profile);
    }

    /// Create and register a profile from a single key (backward compat)
    pub async fn register_single_key(&self, provider_name: &str, key: String) {
        let profile = AuthProfile::single_key(provider_name, key);
        self.register_profile(provider_name, profile).await;
    }

    /// Create and register a profile from config, then load any persisted state.
    pub async fn register_from_config(&self, provider_name: &str, config: &AuthProfileConfig) {
        let keys: Vec<(String, String)> = config
            .keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let label = if k.label.is_empty() {
                    format!("key-{}", i)
                } else {
                    k.label.clone()
                };
                (k.key.clone(), label)
            })
            .collect();

        let profile =
            AuthProfile::with_keys(provider_name, keys, config.cooldown_secs, config.max_failures);
        self.register_profile(provider_name, profile).await;
        self.load(provider_name).await;
    }

    /// Get the current active key for a provider
    pub async fn current_key(&self, provider_name: &str) -> Option<String> {
        let profiles = self.profiles.read().await;
        profiles
            .get(provider_name)
            .and_then(|p| p.current_key().map(String::from))
    }

    /// Rotate to the next available key for a provider. Returns the new key.
    ///
    /// `cooldown_secs` is forwarded to the underlying [`AuthProfile::rotate`].
    /// State is persisted to the store if one is configured.
    pub async fn rotate(&self, provider_name: &str, cooldown_secs: u64) -> Option<String> {
        let mut profiles = self.profiles.write().await;
        let result = profiles
            .get_mut(provider_name)
            .and_then(|p| p.rotate(cooldown_secs));
        drop(profiles);
        if result.is_some() {
            self.save(provider_name).await;
        }
        result
    }

    /// Record success on the current key for a provider.
    /// State is persisted to the store if one is configured.
    pub async fn record_success(&self, provider_name: &str) {
        let mut profiles = self.profiles.write().await;
        if let Some(p) = profiles.get_mut(provider_name) {
            p.record_success();
        }
        drop(profiles);
        self.save(provider_name).await;
    }

    /// Persist the current auth profile state for a provider.
    async fn save(&self, provider_name: &str) {
        let store_opt = { self.store.read().await.clone() };
        if let Some(store) = store_opt {
            let profiles = self.profiles.read().await;
            if let Some(profile) = profiles.get(provider_name) {
                let profile = profile.clone();
                drop(profiles);
                if let Err(e) = store.save_profile_state(provider_name, &profile).await {
                    warn!("Failed to persist auth profile state for {}: {}", provider_name, e);
                }
            }
        }
    }

    /// Load previously persisted auth profile state for a provider.
    async fn load(&self, provider_name: &str) {
        let store_opt = { self.store.read().await.clone() };
        if let Some(store) = store_opt {
            let mut profiles = self.profiles.write().await;
            if let Some(profile) = profiles.get_mut(provider_name) {
                if let Err(e) = store.load_profile_state(provider_name, profile).await {
                    warn!("Failed to load auth profile state for {}: {}", provider_name, e);
                }
            }
        }
    }

    /// Get profile status for a provider
    pub async fn get_status(&self, provider_name: &str) -> Option<ProfileStatus> {
        let profiles = self.profiles.read().await;
        profiles.get(provider_name).map(|p| {
            let current_label = p
                .keys
                .get(p.current_index)
                .map(|k| k.label.clone())
                .unwrap_or_default();
            let current_masked = p
                .keys
                .get(p.current_index)
                .map(|k| k.masked_key())
                .unwrap_or_default();
            ProfileStatus {
                provider_name: p.provider_name.clone(),
                current_key_label: current_label,
                current_key_masked: current_masked,
                total_keys: p.key_count(),
                available_keys: p.available_count(),
                keys: p.key_statuses(),
            }
        })
    }

    /// Get all profile statuses
    pub async fn all_statuses(&self) -> Vec<ProfileStatus> {
        let profiles = self.profiles.read().await;
        profiles
            .values()
            .map(|p| {
                let current_label = p
                    .keys
                    .get(p.current_index)
                    .map(|k| k.label.clone())
                    .unwrap_or_default();
                let current_masked = p
                    .keys
                    .get(p.current_index)
                    .map(|k| k.masked_key())
                    .unwrap_or_default();
                ProfileStatus {
                    provider_name: p.provider_name.clone(),
                    current_key_label: current_label,
                    current_key_masked: current_masked,
                    total_keys: p.key_count(),
                    available_keys: p.available_count(),
                    keys: p.key_statuses(),
                }
            })
            .collect()
    }

    /// Check if an error indicates auth/key rotation should occur.
    ///
    /// Delegates to [`FailureClass`] for structured classification.
    pub fn should_rotate(error: &crate::error::MantaError) -> bool {
        use crate::model_router::failure_class::FailureClass;
        FailureClass::from_error(error, None).should_rotate_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_entry_new() {
        let entry = KeyEntry::new("secret-key".to_string(), "primary");
        assert_eq!(entry.key, "secret-key");
        assert_eq!(entry.label, "primary");
        assert_eq!(entry.status, KeyStatus::Active);
        assert_eq!(entry.failure_count, 0);
        assert_eq!(entry.success_count, 0);
        assert!(entry.last_failure.is_none());
        assert!(entry.cooldown_until.is_none());
    }

    #[test]
    fn test_key_entry_is_available() {
        let mut entry = KeyEntry::new("k".to_string(), "test");
        assert!(entry.is_available());

        entry.status = KeyStatus::Disabled;
        assert!(!entry.is_available());

        entry.status = KeyStatus::Active;
        entry.cooldown_until = Some(Utc::now() + chrono::Duration::seconds(3600));
        assert!(!entry.is_available());

        entry.cooldown_until = Some(Utc::now() - chrono::Duration::seconds(1));
        assert!(entry.is_available());
    }

    #[test]
    fn test_key_entry_record_success() {
        let mut entry = KeyEntry::new("k".to_string(), "test");
        entry.failure_count = 5;
        entry.status = KeyStatus::Cooldown;
        entry.cooldown_until = Some(Utc::now());

        entry.record_success();
        assert_eq!(entry.success_count, 1);
        assert_eq!(entry.failure_count, 0);
        assert_eq!(entry.status, KeyStatus::Active);
        assert!(entry.cooldown_until.is_none());
    }

    #[test]
    fn test_key_entry_record_failure() {
        let mut entry = KeyEntry::new("k".to_string(), "test");
        entry.record_failure(30);
        assert_eq!(entry.failure_count, 1);
        assert!(entry.last_failure.is_some());
        assert_eq!(entry.status, KeyStatus::Cooldown);
        assert!(entry.cooldown_until.is_some());
    }

    #[test]
    fn test_key_entry_masked_key() {
        let entry = KeyEntry::new("very-long-secret-key".to_string(), "test");
        assert_eq!(entry.masked_key(), "very****");

        let entry_short = KeyEntry::new("short".to_string(), "test");
        assert_eq!(entry_short.masked_key(), "****");
    }

    #[test]
    fn test_auth_profile_single_key() {
        let profile = AuthProfile::single_key("openai", "sk-test".to_string());
        assert_eq!(profile.provider_name, "openai");
        assert_eq!(profile.key_count(), 1);
        assert_eq!(profile.available_count(), 1);
        assert_eq!(profile.current_key(), Some("sk-test"));
    }

    #[test]
    fn test_auth_profile_with_keys() {
        let profile = AuthProfile::with_keys(
            "openai",
            vec![
                ("key1".to_string(), "primary"),
                ("key2".to_string(), "secondary"),
            ],
            60,
            3,
        );
        assert_eq!(profile.key_count(), 2);
        assert_eq!(profile.available_count(), 2);
        assert_eq!(profile.current_key(), Some("key1"));
    }

    #[test]
    fn test_auth_profile_rotate() {
        let mut profile = AuthProfile::with_keys(
            "openai",
            vec![
                ("key1".to_string(), "primary"),
                ("key2".to_string(), "secondary"),
            ],
            60,
            3,
        );

        let new_key = profile.rotate(60);
        assert_eq!(new_key, Some("key2".to_string()));
        assert_eq!(profile.current_key(), Some("key2"));

        // Rotate again — key1 is on cooldown, no available keys
        let next = profile.rotate(60);
        assert!(next.is_none());
    }

    #[test]
    fn test_auth_profile_rotate_disables_after_max_failures() {
        let mut profile =
            AuthProfile::with_keys("openai", vec![("key1".to_string(), "primary")], 0, 1);

        profile.rotate(0);
        let statuses = profile.key_statuses();
        assert_eq!(statuses[0].status, KeyStatus::Disabled);
    }

    #[test]
    fn test_auth_profile_record_success() {
        let mut profile = AuthProfile::single_key("openai", "sk-test".to_string());
        profile.record_success();

        let statuses = profile.key_statuses();
        assert_eq!(statuses[0].success_count, 1);
        assert_eq!(statuses[0].failure_count, 0);
    }

    #[test]
    fn test_auth_profile_key_statuses() {
        let profile = AuthProfile::with_keys(
            "openai",
            vec![("key1".to_string(), "primary"), ("key2".to_string(), "")],
            60,
            3,
        );

        let statuses = profile.key_statuses();
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].label, "primary");
        assert_eq!(statuses[1].label, "key-1");
    }

    #[tokio::test]
    async fn test_auth_profile_manager_basic() {
        let manager = AuthProfileManager::new();
        manager
            .register_single_key("openai", "sk-test".to_string())
            .await;

        let key = manager.current_key("openai").await;
        assert_eq!(key, Some("sk-test".to_string()));

        let status = manager.get_status("openai").await.unwrap();
        assert_eq!(status.provider_name, "openai");
        assert_eq!(status.total_keys, 1);

        manager.record_success("openai").await;
        let status = manager.get_status("openai").await.unwrap();
        assert_eq!(status.keys[0].success_count, 1);
    }

    #[tokio::test]
    async fn test_auth_profile_manager_rotate() {
        let manager = AuthProfileManager::new();
        let profile = AuthProfile::with_keys(
            "openai",
            vec![
                ("key1".to_string(), "primary"),
                ("key2".to_string(), "secondary"),
            ],
            60,
            3,
        );
        manager.register_profile("openai", profile).await;

        let new_key = manager.rotate("openai", 60).await;
        assert_eq!(new_key, Some("key2".to_string()));
    }

    #[test]
    fn test_should_rotate_429() {
        let err = crate::error::MantaError::Internal("429 rate limit".to_string());
        assert!(AuthProfileManager::should_rotate(&err));
    }

    #[test]
    fn test_should_rotate_401() {
        let err = crate::error::MantaError::Internal("401 unauthorized".to_string());
        assert!(AuthProfileManager::should_rotate(&err));
    }

    #[test]
    fn test_should_not_rotate_403() {
        // 403 is classified as AuthPermanent — should disable key, not rotate
        let err = crate::error::MantaError::Internal("403 forbidden".to_string());
        assert!(!AuthProfileManager::should_rotate(&err));
    }

    #[test]
    fn test_should_not_rotate_other_error() {
        let err = crate::error::MantaError::Internal("connection timeout".to_string());
        assert!(!AuthProfileManager::should_rotate(&err));
    }

    #[test]
    fn test_auth_profile_config_defaults() {
        let config = AuthProfileConfig::default();
        assert_eq!(config.cooldown_secs, 60);
        assert_eq!(config.max_failures, 3);
        assert!(config.keys.is_empty());
    }
}
