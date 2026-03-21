//! DM Pairing and Access Control
//!
//! Implements a one-time pairing flow that authorizes new users to interact
//! with the assistant over a specific channel.  The flow:
//!
//! 1. Admin generates a `PairingCode` (6-digit OTP, short TTL).
//! 2. User sends the code via DM on the target channel.
//! 3. `PairingStore::redeem` validates and activates the user.
//! 4. Future `is_authorized` checks consult the store.
//!
//! # Example
//!
//! ```rust
//! use manta::security::pairing::{PairingStore, PairingRequest};
//! use std::time::Duration;
//!
//! # async fn example() {
//! let store = PairingStore::new();
//!
//! // Admin creates a code
//! let req = PairingRequest {
//!     channel: "telegram".into(),
//!     user_id: "123456".into(),
//!     ttl: Duration::from_secs(3600),
//! };
//! let code = store.generate(&req).await;
//!
//! // User redeems it
//! let ok = store.redeem("telegram", "123456", &code).await;
//! assert!(ok);
//!
//! // User is now authorized
//! assert!(store.is_authorized("telegram", "123456").await);
//! # }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

// ── Request ───────────────────────────────────────────────────────────────────

/// Parameters for generating a new pairing code.
#[derive(Debug, Clone)]
pub struct PairingRequest {
    /// Target channel (e.g. `"telegram"`, `"discord"`).
    pub channel: String,
    /// User ID that will redeem the code.
    pub user_id: String,
    /// How long the code is valid.
    pub ttl: Duration,
}

// ── Stored entries ────────────────────────────────────────────────────────────

/// A pending (unredeemed) pairing code.
#[derive(Debug, Clone)]
struct PendingCode {
    code: String,
    channel: String,
    user_id: String,
    expires_at: SystemTime,
}

/// A successfully authorized user entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizedUser {
    /// Channel the user is authorized on.
    pub channel: String,
    /// User ID.
    pub user_id: String,
    /// When authorization was granted.
    pub authorized_at: SystemTime,
    /// When authorization expires (`None` = never).
    pub expires_at: Option<SystemTime>,
    /// Who generated the pairing code (admin user ID).
    pub paired_by: Option<String>,
}

impl AuthorizedUser {
    /// Return `true` if the authorization has not yet expired.
    pub fn is_valid(&self) -> bool {
        self.expires_at.map(|exp| SystemTime::now() < exp).unwrap_or(true)
    }
}

// ── Store ─────────────────────────────────────────────────────────────────────

/// Thread-safe store for pairing codes and authorized users.
#[derive(Debug, Clone)]
pub struct PairingStore {
    pending: Arc<RwLock<Vec<PendingCode>>>,
    authorized: Arc<RwLock<HashMap<(String, String), AuthorizedUser>>>,
}

impl Default for PairingStore {
    fn default() -> Self {
        Self {
            pending: Arc::new(RwLock::new(Vec::new())),
            authorized: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl PairingStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self::default()
    }

    // ── Code generation ───────────────────────────────────────────────────────

    /// Generate a new pairing code for `req`.
    ///
    /// Returns the plaintext code that the admin should share with the user.
    /// Any previous pending code for the same (channel, user_id) pair is
    /// replaced.
    pub async fn generate(&self, req: &PairingRequest) -> String {
        let code = generate_otp();
        let expires_at = SystemTime::now() + req.ttl;

        let mut pending = self.pending.write().await;

        // Replace existing pending entry for the same (channel, user).
        pending.retain(|p| !(p.channel == req.channel && p.user_id == req.user_id));

        pending.push(PendingCode {
            code: code.clone(),
            channel: req.channel.clone(),
            user_id: req.user_id.clone(),
            expires_at,
        });

        info!(
            channel = %req.channel,
            user_id = %req.user_id,
            "Pairing code generated (expires in {}s)",
            req.ttl.as_secs()
        );

        code
    }

    // ── Redemption ────────────────────────────────────────────────────────────

    /// Attempt to redeem a pairing code.
    ///
    /// Returns `true` and activates the user when the code is valid and
    /// unexpired.  The code is consumed (one-time use) whether or not it is
    /// expired.
    pub async fn redeem(
        &self,
        channel: &str,
        user_id: &str,
        code: &str,
    ) -> bool {
        let mut pending = self.pending.write().await;
        let now = SystemTime::now();

        let pos = pending.iter().position(|p| {
            p.channel == channel && p.user_id == user_id && p.code == code
        });

        let Some(idx) = pos else {
            warn!(channel, user_id, "Pairing code not found or wrong user/channel");
            return false;
        };

        let entry = pending.remove(idx);

        if now > entry.expires_at {
            warn!(channel, user_id, "Pairing code expired");
            return false;
        }

        // Authorize the user.
        let authorized = AuthorizedUser {
            channel: channel.to_string(),
            user_id: user_id.to_string(),
            authorized_at: now,
            expires_at: None, // permanent unless revoked
            paired_by: None,
        };

        drop(pending); // release write lock before acquiring authorized write lock

        let mut auth = self.authorized.write().await;
        auth.insert((channel.to_string(), user_id.to_string()), authorized);

        info!(channel, user_id, "User paired and authorized");
        true
    }

    // ── Access checks ─────────────────────────────────────────────────────────

    /// Return `true` if `user_id` is currently authorized on `channel`.
    pub async fn is_authorized(&self, channel: &str, user_id: &str) -> bool {
        let auth = self.authorized.read().await;
        auth.get(&(channel.to_string(), user_id.to_string()))
            .map(|u| u.is_valid())
            .unwrap_or(false)
    }

    /// List all currently valid authorizations.
    pub async fn list_authorized(&self) -> Vec<AuthorizedUser> {
        let auth = self.authorized.read().await;
        auth.values().filter(|u| u.is_valid()).cloned().collect()
    }

    /// List authorized users filtered by channel.
    pub async fn list_authorized_for_channel(&self, channel: &str) -> Vec<AuthorizedUser> {
        let auth = self.authorized.read().await;
        auth.values()
            .filter(|u| u.channel == channel && u.is_valid())
            .cloned()
            .collect()
    }

    // ── Revocation ────────────────────────────────────────────────────────────

    /// Revoke access for `user_id` on `channel`.
    ///
    /// Returns `true` if an entry was removed.
    pub async fn revoke(&self, channel: &str, user_id: &str) -> bool {
        let mut auth = self.authorized.write().await;
        let removed = auth.remove(&(channel.to_string(), user_id.to_string())).is_some();
        if removed {
            info!(channel, user_id, "User access revoked");
        }
        removed
    }

    // ── Maintenance ───────────────────────────────────────────────────────────

    /// Remove expired pending codes and expired authorizations.
    pub async fn sweep_expired(&self) {
        let now = SystemTime::now();

        let mut pending = self.pending.write().await;
        let before = pending.len();
        pending.retain(|p| now < p.expires_at);
        let removed = before - pending.len();
        drop(pending);

        let mut auth = self.authorized.write().await;
        auth.retain(|_, u| u.is_valid());

        debug!("Pairing store sweep: removed {} expired codes", removed);
    }
}

// ── OTP generation ────────────────────────────────────────────────────────────

/// Generate a 6-digit numeric OTP.
fn generate_otp() -> String {
    let n: u32 = rand::random::<u32>() % 900_000 + 100_000;
    n.to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_generate_and_redeem() {
        let store = PairingStore::new();
        let req = PairingRequest {
            channel: "telegram".into(),
            user_id: "alice".into(),
            ttl: Duration::from_secs(3600),
        };
        let code = store.generate(&req).await;
        assert_eq!(code.len(), 6);

        let ok = store.redeem("telegram", "alice", &code).await;
        assert!(ok);
        assert!(store.is_authorized("telegram", "alice").await);
    }

    #[tokio::test]
    async fn test_wrong_code_rejected() {
        let store = PairingStore::new();
        let req = PairingRequest {
            channel: "telegram".into(),
            user_id: "bob".into(),
            ttl: Duration::from_secs(3600),
        };
        store.generate(&req).await;

        let ok = store.redeem("telegram", "bob", "000000").await;
        assert!(!ok);
        assert!(!store.is_authorized("telegram", "bob").await);
    }

    #[tokio::test]
    async fn test_code_is_one_time_use() {
        let store = PairingStore::new();
        let req = PairingRequest {
            channel: "discord".into(),
            user_id: "carol".into(),
            ttl: Duration::from_secs(3600),
        };
        let code = store.generate(&req).await;

        assert!(store.redeem("discord", "carol", &code).await);
        // Second redemption must fail (code consumed).
        assert!(!store.redeem("discord", "carol", &code).await);
    }

    #[tokio::test]
    async fn test_revoke() {
        let store = PairingStore::new();
        let req = PairingRequest {
            channel: "slack".into(),
            user_id: "dave".into(),
            ttl: Duration::from_secs(3600),
        };
        let code = store.generate(&req).await;
        store.redeem("slack", "dave", &code).await;

        assert!(store.is_authorized("slack", "dave").await);
        store.revoke("slack", "dave").await;
        assert!(!store.is_authorized("slack", "dave").await);
    }

    #[tokio::test]
    async fn test_list_authorized() {
        let store = PairingStore::new();
        for user in ["u1", "u2"] {
            let code = store
                .generate(&PairingRequest {
                    channel: "telegram".into(),
                    user_id: user.into(),
                    ttl: Duration::from_secs(3600),
                })
                .await;
            store.redeem("telegram", user, &code).await;
        }

        let list = store.list_authorized_for_channel("telegram").await;
        assert_eq!(list.len(), 2);
    }
}
