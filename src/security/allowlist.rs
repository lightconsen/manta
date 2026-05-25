//! Multi-dimensional allowlist with compiled HashSet cache and wildcard support.
//!
//! Allows fine-grained access control across multiple identity dimensions:
//! ID, username, display name, tag, E164 phone number, prefixed identifiers,
//! slug, and localpart.
//!
//! # Example
//!
//! ```rust
//! use manta::security::allowlist::{Allowlist, AllowlistEntry, MatchSource};
//!
//! # async fn example() {
//! let mut allowlist = Allowlist::new();
//!
//! // Add by user ID
//! allowlist.add(AllowlistEntry {
//!     id: "allow-1".to_string(),
//!     sources: vec![MatchSource::Id("u123".to_string())],
//!     account_id: Some("acme".to_string()),
//!     channel_id: Some("telegram".to_string()),
//!     group_id: None,
//! });
//!
//! // Add by username with wildcard pattern
//! allowlist.add(AllowlistEntry {
//!     id: "allow-2".to_string(),
//!     sources: vec![
//!         MatchSource::Username("admin".to_string()),
//!         MatchSource::Username("*_bot".to_string()),  // wildcard
//!     ],
//!     account_id: None,
//!     channel_id: None,
//!     group_id: None,
//! });
//!
//! // Check if a user is allowed
//! let is_ok = allowlist.is_allowed("u123").await;
//! assert!(is_ok);
//! # }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Match source — how an identity is matched against the allowlist.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "type", content = "value")]
pub enum MatchSource {
    /// Exact user ID match (e.g., "u123")
    Id(String),
    /// Username match (e.g., "@alice")
    Username(String),
    /// Display name match (e.g., "Alice Smith")
    Name(String),
    /// Tag match (e.g., "#vip")
    Tag(String),
    /// E164 phone number match (e.g., "+1234567890")
    E164(String),
    /// Prefixed ID (e.g., prefix="tg", id="u123" → "tg:u123")
    PrefixedId { prefix: String, id: String },
    /// Prefixed username (e.g., prefix="tg", user="alice")
    PrefixedUser { prefix: String, user: String },
    /// Prefixed display name
    PrefixedName { prefix: String, name: String },
    /// Slug match (URL-safe identifier)
    Slug(String),
    /// Localpart match (e.g., "alice" from "alice@example.com")
    Localpart(String),
}

impl MatchSource {
    /// Check if this source matches the given value.
    ///
    /// Supports wildcard `*` patterns for string matching.
    pub fn matches(&self, value: &str) -> bool {
        let pattern = match self {
            MatchSource::Id(v) => v,
            MatchSource::Username(v) => v,
            MatchSource::Name(v) => v,
            MatchSource::Tag(v) => v,
            MatchSource::E164(v) => v,
            MatchSource::PrefixedId { prefix, id } => {
                let combined = format!("{}:{}", prefix, id);
                return combined == value || wildcard_match(&combined, value);
            }
            MatchSource::PrefixedUser { prefix, user } => {
                let combined = format!("{}:{}", prefix, user);
                return combined == value || wildcard_match(&combined, value);
            }
            MatchSource::PrefixedName { prefix, name } => {
                let combined = format!("{}:{}", prefix, name);
                return combined == value || wildcard_match(&combined, value);
            }
            MatchSource::Slug(v) => v,
            MatchSource::Localpart(v) => v,
        };
        pattern == value || wildcard_match(pattern, value)
    }

    /// Get the string value of this source (for compiled cache keys).
    pub fn as_str(&self) -> &str {
        match self {
            MatchSource::Id(v) => v,
            MatchSource::Username(v) => v,
            MatchSource::Name(v) => v,
            MatchSource::Tag(v) => v,
            MatchSource::E164(v) => v,
            MatchSource::PrefixedId { prefix, id } => {
                // We store the combined string for cache purposes
                // This is a workaround since we can't borrow from a temporary
                static EMPTY: &str = "";
                let _ = (prefix, id);
                EMPTY
            }
            MatchSource::PrefixedUser { prefix, user } => {
                static EMPTY: &str = "";
                let _ = (prefix, user);
                EMPTY
            }
            MatchSource::PrefixedName { prefix, name } => {
                static EMPTY: &str = "";
                let _ = (prefix, name);
                EMPTY
            }
            MatchSource::Slug(v) => v,
            MatchSource::Localpart(v) => v,
        }
    }

    /// Get a cache key for this source.
    pub fn cache_key(&self) -> String {
        match self {
            MatchSource::Id(v)
            | MatchSource::Username(v)
            | MatchSource::Name(v)
            | MatchSource::Tag(v)
            | MatchSource::E164(v)
            | MatchSource::Slug(v)
            | MatchSource::Localpart(v) => v.clone(),
            MatchSource::PrefixedId { prefix, id } => format!("{}:{}", prefix, id),
            MatchSource::PrefixedUser { prefix, user } => format!("{}:{}", prefix, user),
            MatchSource::PrefixedName { prefix, name } => format!("{}:{}", prefix, name),
        }
    }
}

/// Simple wildcard matching: `*` matches any sequence of characters.
fn wildcard_match(pattern: &str, value: &str) -> bool {
    if !pattern.contains('*') {
        return false;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        // Just "*" — matches everything
        return parts[0].is_empty();
    }
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            // First part must match at the start
            if !value.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else {
            // Subsequent parts can match anywhere after the previous match
            if let Some(idx) = value[pos..].find(part) {
                pos = pos + idx + part.len();
            } else {
                return false;
            }
        }
    }
    // If pattern doesn't end with *, the value must end with the last part
    if !pattern.ends_with('*') && pos != value.len() {
        // Check if the last part matches at the end
        if let Some(last) = parts.last() {
            if !value.ends_with(last) {
                return false;
            }
        }
    }
    true
}

/// A single allowlist entry with multi-dimensional matching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistEntry {
    /// Unique identifier for this entry.
    pub id: String,
    /// Match sources — any match grants access (OR logic).
    pub sources: Vec<MatchSource>,
    /// Optional account scope (None = global).
    pub account_id: Option<String>,
    /// Optional channel scope (None = all channels).
    pub channel_id: Option<String>,
    /// Optional group scope (None = all groups).
    pub group_id: Option<String>,
}

impl AllowlistEntry {
    /// Create a new entry with a single ID match source.
    pub fn by_id(id: &str, value: &str) -> Self {
        Self {
            id: id.to_string(),
            sources: vec![MatchSource::Id(value.to_string())],
            account_id: None,
            channel_id: None,
            group_id: None,
        }
    }

    /// Create a new entry with a username match source.
    pub fn by_username(id: &str, username: &str) -> Self {
        Self {
            id: id.to_string(),
            sources: vec![MatchSource::Username(username.to_string())],
            account_id: None,
            channel_id: None,
            group_id: None,
        }
    }

    /// Create a new entry with an E164 phone number match source.
    pub fn by_e164(id: &str, phone: &str) -> Self {
        Self {
            id: id.to_string(),
            sources: vec![MatchSource::E164(phone.to_string())],
            account_id: None,
            channel_id: None,
            group_id: None,
        }
    }
}

/// Thread-safe multi-dimensional allowlist.
///
/// Maintains a compiled `HashSet` cache for O(1) exact matching,
/// plus a list of wildcard patterns that require runtime evaluation.
pub struct Allowlist {
    /// All entries.
    entries: Arc<RwLock<Vec<AllowlistEntry>>>,
    /// Compiled cache of exact-match keys for O(1) lookup.
    /// Contains lowercase normalized values for case-insensitive matching.
    compiled: Arc<RwLock<HashSet<String>>>,
    /// Wildcard patterns that need runtime evaluation.
    wildcard_patterns: Arc<RwLock<Vec<(String, String)>>>, // (pattern, entry_id)
}

impl Default for Allowlist {
    fn default() -> Self {
        Self::new()
    }
}

// Clone is cheap (just Arc clones)
impl Clone for Allowlist {
    fn clone(&self) -> Self {
        Self {
            entries: Arc::clone(&self.entries),
            compiled: Arc::clone(&self.compiled),
            wildcard_patterns: Arc::clone(&self.wildcard_patterns),
        }
    }
}

impl std::fmt::Debug for Allowlist {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Allowlist")
            .field("entry_count", &self.entries.blocking_read().len())
            .finish()
    }
}

impl Allowlist {
    /// Create a new empty allowlist.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(Vec::new())),
            compiled: Arc::new(RwLock::new(HashSet::new())),
            wildcard_patterns: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add an entry to the allowlist.
    ///
    /// Rebuilds the compiled cache for O(1) lookup.
    pub async fn add(&self, entry: AllowlistEntry) {
        let mut entries = self.entries.write().await;

        // Check if entry with same ID already exists
        if entries.iter().any(|e| e.id == entry.id) {
            warn!("Allowlist entry '{}' already exists, skipping", entry.id);
            return;
        }

        entries.push(entry);
        drop(entries);

        self.rebuild_cache().await;
        info!("Allowlist entry added");
    }

    /// Remove an entry by ID.
    pub async fn remove(&self, id: &str) -> bool {
        let mut entries = self.entries.write().await;
        let before = entries.len();
        entries.retain(|e| e.id != id);
        let removed = entries.len() < before;
        drop(entries);

        if removed {
            self.rebuild_cache().await;
            info!("Allowlist entry '{}' removed", id);
        }
        removed
    }

    /// Check if a value is allowed by any match source.
    ///
    /// First checks the compiled exact-match cache, then evaluates wildcard patterns.
    pub async fn is_allowed(&self, value: &str) -> bool {
        let normalized = value.to_lowercase();

        // Check compiled cache first (O(1))
        {
            let cache = self.compiled.read().await;
            if cache.contains(&normalized) {
                return true;
            }
        }

        // Check wildcard patterns (O(n) but typically small)
        let patterns = self.wildcard_patterns.read().await;
        for (pattern, _entry_id) in patterns.iter() {
            if wildcard_match(pattern, &normalized) {
                debug!("Allowlist wildcard match: {} matched {}", value, pattern);
                return true;
            }
        }

        false
    }

    /// Check if a value is allowed within a specific scope.
    ///
    /// Only considers entries that match the given scope constraints.
    pub async fn is_allowed_in_scope(
        &self,
        value: &str,
        account_id: Option<&str>,
        channel_id: Option<&str>,
        group_id: Option<&str>,
    ) -> bool {
        let entries = self.entries.read().await;
        let normalized = value.to_lowercase();

        for entry in entries.iter() {
            // Scope check
            if let Some(acc) = entry.account_id.as_deref() {
                if account_id != Some(acc) {
                    continue;
                }
            }
            if let Some(ch) = entry.channel_id.as_deref() {
                if channel_id != Some(ch) {
                    continue;
                }
            }
            if let Some(grp) = entry.group_id.as_deref() {
                if group_id != Some(grp) {
                    continue;
                }
            }

            // Match check
            for source in &entry.sources {
                if source.matches(&normalized) {
                    return true;
                }
            }
        }

        false
    }

    /// Get all entries.
    pub async fn list(&self) -> Vec<AllowlistEntry> {
        self.entries.read().await.clone()
    }

    /// Get entry count.
    pub async fn len(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Check if allowlist is empty.
    pub async fn is_empty(&self) -> bool {
        self.entries.read().await.is_empty()
    }

    /// Save allowlist to a JSON file.
    pub async fn save(&self, path: &Path) -> crate::Result<()> {
        let entries = self.entries.read().await;
        let json = serde_json::to_string_pretty(&*entries)?;

        // Write with file locking to prevent concurrent writes
        #[cfg(unix)]
        {
            use std::fs::File;
            use std::io::Write;
            use std::os::unix::io::AsRawFd;

            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }

            let file = File::create(path)?;
            let lock_result = unsafe {
                libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
            };
            if lock_result != 0 {
                warn!("Could not acquire file lock on {:?}, proceeding anyway", path);
            }

            let mut file = file;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
        }

        #[cfg(not(unix))]
        {
            std::fs::write(path, json)?;
        }

        info!("Allowlist saved to {:?}", path);
        Ok(())
    }

    /// Load allowlist from a JSON file.
    pub async fn load(&self, path: &Path) -> crate::Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(path)?;
        let entries: Vec<AllowlistEntry> = serde_json::from_str(&content)?;

        let mut entries_guard = self.entries.write().await;
        *entries_guard = entries;
        drop(entries_guard);

        self.rebuild_cache().await;
        info!("Allowlist loaded from {:?} ({} entries)", path, self.len().await);
        Ok(())
    }

    /// Rebuild the compiled cache and wildcard pattern list.
    async fn rebuild_cache(&self) {
        let entries = self.entries.read().await;
        let mut compiled = HashSet::new();
        let mut wildcards = Vec::new();

        for entry in entries.iter() {
            for source in &entry.sources {
                let key = source.cache_key();
                if key.contains('*') {
                    wildcards.push((key.to_lowercase(), entry.id.clone()));
                } else {
                    compiled.insert(key.to_lowercase());
                }
            }
        }

        drop(entries);

        let mut compiled_guard = self.compiled.write().await;
        *compiled_guard = compiled;
        drop(compiled_guard);

        let mut wildcard_guard = self.wildcard_patterns.write().await;
        *wildcard_guard = wildcards;
    }
}

/// Backward compatibility: simple ID-based allowlist wrapper.
///
/// Wraps a `PairingStore`-style `HashMap<(channel, user_id), AuthorizedUser>`
/// with the new multi-dimensional `Allowlist` for gradual migration.
pub async fn is_user_allowed(
    allowlist: &Allowlist,
    channel: &str,
    user_id: &str,
    username: Option<&str>,
) -> bool {
    // Check by user ID (primary identifier)
    if allowlist.is_allowed(user_id).await {
        return true;
    }

    // Check by prefixed ID (channel:user_id format)
    let prefixed = format!("{}:{}", channel, user_id);
    if allowlist.is_allowed(&prefixed).await {
        return true;
    }

    // Check by username if available
    if let Some(u) = username {
        if allowlist.is_allowed(u).await {
            return true;
        }
        // Check prefixed username
        let prefixed_user = format!("{}:{}", channel, u);
        if allowlist.is_allowed(&prefixed_user).await {
            return true;
        }
    }

    // Check scoped match
    allowlist
        .is_allowed_in_scope(user_id, None, Some(channel), None)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_match_simple() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("admin*", "admin"));
        assert!(wildcard_match("admin*", "admin_bot"));
        assert!(wildcard_match("*_bot", "my_bot"));
        assert!(!wildcard_match("*_bot", "bot")); // "_bot" suffix required
        assert!(!wildcard_match("admin*", "user"));
        assert!(!wildcard_match("*_bot", "admin_user"));
    }

    #[test]
    fn test_wildcard_match_multiple() {
        assert!(wildcard_match("*@*.ts.net", "user@mytailnet.ts.net"));
        assert!(wildcard_match("u*", "u123"));
        assert!(!wildcard_match("x*", "u123"));
    }

    #[test]
    fn test_match_source_matches() {
        assert!(MatchSource::Id("u123".to_string()).matches("u123"));
        assert!(!MatchSource::Id("u123".to_string()).matches("u456"));

        assert!(MatchSource::Username("*_admin".to_string()).matches("bot_admin"));

        assert!(MatchSource::PrefixedId {
            prefix: "tg".to_string(),
            id: "u123".to_string()
        }
        .matches("tg:u123"));
    }

    #[tokio::test]
    async fn test_allowlist_basic() {
        let allowlist = Allowlist::new();
        assert!(allowlist.is_empty().await);

        allowlist
            .add(AllowlistEntry::by_id("entry-1", "u123"))
            .await;
        assert_eq!(allowlist.len().await, 1);
        assert!(!allowlist.is_empty().await);

        assert!(allowlist.is_allowed("u123").await);
        assert!(!allowlist.is_allowed("u456").await);
    }

    #[tokio::test]
    async fn test_allowlist_wildcard() {
        let allowlist = Allowlist::new();
        allowlist
            .add(AllowlistEntry {
                id: "wildcard-1".to_string(),
                sources: vec![MatchSource::Username("*_bot".to_string())],
                account_id: None,
                channel_id: None,
                group_id: None,
            })
            .await;

        assert!(allowlist.is_allowed("my_bot").await);
        assert!(allowlist.is_allowed("admin_bot").await);
        assert!(!allowlist.is_allowed("my_admin").await);
    }

    #[tokio::test]
    async fn test_allowlist_remove() {
        let allowlist = Allowlist::new();
        allowlist
            .add(AllowlistEntry::by_id("entry-1", "u123"))
            .await;
        assert!(allowlist.is_allowed("u123").await);

        assert!(allowlist.remove("entry-1").await);
        assert!(!allowlist.is_allowed("u123").await);
        assert!(!allowlist.remove("nonexistent").await);
    }

    #[tokio::test]
    async fn test_allowlist_scoped() {
        let allowlist = Allowlist::new();
        allowlist
            .add(AllowlistEntry {
                id: "scoped-1".to_string(),
                sources: vec![MatchSource::Id("u123".to_string())],
                account_id: Some("acme".to_string()),
                channel_id: Some("telegram".to_string()),
                group_id: None,
            })
            .await;

        // Exact scope match
        assert!(
            allowlist
                .is_allowed_in_scope("u123", Some("acme"), Some("telegram"), None)
                .await
        );

        // Wrong account
        assert!(
            !allowlist
                .is_allowed_in_scope("u123", Some("other"), Some("telegram"), None)
                .await
        );

        // Wrong channel
        assert!(
            !allowlist
                .is_allowed_in_scope("u123", Some("acme"), Some("discord"), None)
                .await
        );
    }

    #[tokio::test]
    async fn test_is_user_allowed() {
        let allowlist = Allowlist::new();
        allowlist
            .add(AllowlistEntry::by_id("entry-1", "u123"))
            .await;
        allowlist
            .add(AllowlistEntry {
                id: "entry-2".to_string(),
                sources: vec![MatchSource::PrefixedId {
                    prefix: "telegram".to_string(),
                    id: "u456".to_string(),
                }],
                account_id: None,
                channel_id: None,
                group_id: None,
            })
            .await;

        assert!(is_user_allowed(&allowlist, "telegram", "u123", None).await);
        assert!(is_user_allowed(&allowlist, "telegram", "u456", None).await);
        assert!(!is_user_allowed(&allowlist, "telegram", "u789", None).await);
    }

    #[tokio::test]
    async fn test_allowlist_save_and_load() {
        let allowlist = Allowlist::new();
        allowlist
            .add(AllowlistEntry::by_id("entry-1", "u123"))
            .await;
        allowlist
            .add(AllowlistEntry::by_username("entry-2", "admin"))
            .await;

        let path = std::env::temp_dir().join("manta_allowlist_test.json");
        allowlist.save(&path).await.unwrap();

        let allowlist2 = Allowlist::new();
        allowlist2.load(&path).await.unwrap();

        assert_eq!(allowlist2.len().await, 2);
        assert!(allowlist2.is_allowed("u123").await);
        assert!(allowlist2.is_allowed("admin").await);

        std::fs::remove_file(&path).ok();
    }
}
