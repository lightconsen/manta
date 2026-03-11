//! Security module for Manta
//!
//! Provides authentication, authorization, rate limiting, and sandboxing.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Unique identifier for a user
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);

impl UserId {
    /// Create a new user ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// User information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    /// User ID
    pub id: UserId,
    /// Display name
    pub name: String,
    /// When the user was created
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Whether the user is an admin
    pub is_admin: bool,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl User {
    /// Create a new user
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: UserId::new(id),
            name: name.into(),
            created_at: chrono::Utc::now(),
            is_admin: false,
            metadata: HashMap::new(),
        }
    }

    /// Set admin status
    pub fn admin(mut self, is_admin: bool) -> Self {
        self.is_admin = is_admin;
        self
    }
}

/// Authentication manager
#[derive(Debug, Default)]
pub struct AuthManager {
    /// Registered users
    users: Arc<RwLock<HashMap<UserId, User>>>,
    /// Active sessions
    sessions: Arc<RwLock<HashMap<String, Session>>>,
    /// Whether pairing is required for new users
    pairing_required: bool,
}

/// Session information
#[derive(Debug, Clone)]
pub struct Session {
    /// Session token
    pub token: String,
    /// User ID
    pub user_id: UserId,
    /// When the session was created
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the session expires
    pub expires_at: chrono::DateTime<chrono::Utc>,
    /// Device fingerprint
    pub device_fingerprint: Option<String>,
}

impl AuthManager {
    /// Create a new auth manager
    pub fn new() -> Self {
        Self::default()
    }

    /// Require pairing for new users
    pub fn with_pairing_required(mut self, required: bool) -> Self {
        self.pairing_required = required;
        self
    }

    /// Register a new user
    pub async fn register_user(&self, user: User) -> crate::Result<()> {
        let mut users = self.users.write().await;
        if users.contains_key(&user.id) {
            return Err(crate::error::MantaError::Validation(format!(
                "User {} already exists",
                user.id
            )));
        }
        info!("Registered user: {}", user.id);
        users.insert(user.id.clone(), user);
        Ok(())
    }

    /// Get a user by ID
    pub async fn get_user(&self, user_id: &UserId) -> Option<User> {
        let users = self.users.read().await;
        users.get(user_id).cloned()
    }

    /// Check if a user exists
    pub async fn user_exists(&self, user_id: &UserId) -> bool {
        let users = self.users.read().await;
        users.contains_key(user_id)
    }

    /// Create a new session
    pub async fn create_session(
        &self,
        user_id: UserId,
        ttl_hours: i64,
    ) -> crate::Result<Session> {
        // Verify user exists
        if !self.user_exists(&user_id).await {
            return Err(crate::error::MantaError::Validation(format!(
                "User {} not found",
                user_id
            )));
        }

        let token = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let session = Session {
            token: token.clone(),
            user_id: user_id.clone(),
            created_at: now,
            expires_at: now + chrono::Duration::hours(ttl_hours),
            device_fingerprint: None,
        };

        let mut sessions = self.sessions.write().await;
        sessions.insert(token, session.clone());

        debug!("Created session for user: {}", user_id);
        Ok(session)
    }

    /// Validate a session token
    pub async fn validate_session(&self, token: &str) -> Option<Session> {
        let sessions = self.sessions.read().await;
        sessions.get(token).cloned().filter(|s| s.expires_at > chrono::Utc::now())
    }

    /// Revoke a session
    pub async fn revoke_session(&self, token: &str) -> bool {
        let mut sessions = self.sessions.write().await;
        sessions.remove(token).is_some()
    }

    /// Generate a pairing code (simplified implementation)
    pub fn generate_pairing_code(&self) -> String {
        // Generate a 6-digit code
        let code: u32 = rand::random::<u32>() % 900000 + 100000;
        code.to_string()
    }
}

/// Allowlist for controlling access
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    /// Allowed user IDs
    users: Arc<RwLock<HashMap<UserId, AllowlistEntry>>>,
    /// Allowed IP addresses
    ips: Arc<RwLock<Vec<IpAddr>>>,
    /// Default allow policy
    default_allow: bool,
}

/// Allowlist entry for a user
#[derive(Debug, Clone)]
pub struct AllowlistEntry {
    /// User ID
    pub user_id: UserId,
    /// When access was granted
    pub granted_at: chrono::DateTime<chrono::Utc>,
    /// When access expires (None = never)
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Who granted access
    pub granted_by: Option<String>,
    /// Reason for access
    pub reason: Option<String>,
}

impl Allowlist {
    /// Create a new allowlist
    pub fn new() -> Self {
        Self::default()
    }

    /// Set default allow policy
    pub fn with_default_allow(mut self, allow: bool) -> Self {
        self.default_allow = allow;
        self
    }

    /// Add a user to the allowlist
    pub async fn allow_user(
        &self,
        user_id: UserId,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        granted_by: Option<String>,
        reason: Option<String>,
    ) {
        let mut users = self.users.write().await;
        users.insert(
            user_id.clone(),
            AllowlistEntry {
                user_id,
                granted_at: chrono::Utc::now(),
                expires_at,
                granted_by,
                reason,
            },
        );
    }

    /// Remove a user from the allowlist
    pub async fn deny_user(&self, user_id: &UserId) -> bool {
        let mut users = self.users.write().await;
        users.remove(user_id).is_some()
    }

    /// Check if a user is allowed
    pub async fn is_allowed(&self, user_id: &UserId) -> bool {
        if self.default_allow {
            return true;
        }

        let users = self.users.read().await;
        match users.get(user_id) {
            Some(entry) => {
                if let Some(expires) = entry.expires_at {
                    chrono::Utc::now() < expires
                } else {
                    true
                }
            }
            None => false,
        }
    }

    /// Add an IP to the allowlist
    pub async fn allow_ip(&self, ip: IpAddr) {
        let mut ips = self.ips.write().await;
        if !ips.contains(&ip) {
            ips.push(ip);
        }
    }

    /// Check if an IP is allowed
    pub async fn is_ip_allowed(&self, ip: &IpAddr) -> bool {
        let ips = self.ips.read().await;
        ips.is_empty() || ips.contains(ip)
    }

    /// List all allowed users
    pub async fn list_allowed_users(&self) -> Vec<AllowlistEntry> {
        let users = self.users.read().await;
        users.values().cloned().collect()
    }
}

/// Rate limiter using token bucket algorithm
#[derive(Debug, Clone)]
pub struct RateLimiter {
    /// Buckets per user
    buckets: Arc<RwLock<HashMap<UserId, TokenBucket>>>,
    /// Bucket capacity (tokens)
    capacity: u32,
    /// Refill rate (tokens per second)
    refill_rate: f64,
}

/// Token bucket for rate limiting
#[derive(Debug, Clone)]
struct TokenBucket {
    /// Current tokens
    tokens: f64,
    /// Last refill time
    last_refill: chrono::DateTime<chrono::Utc>,
    /// Capacity
    capacity: f64,
    /// Refill rate (tokens per second)
    refill_rate: f64,
}

impl TokenBucket {
    /// Create a new bucket
    fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            tokens: capacity,
            last_refill: chrono::Utc::now(),
            capacity,
            refill_rate,
        }
    }

    /// Refill tokens based on elapsed time
    fn refill(&mut self) {
        let now = chrono::Utc::now();
        let elapsed = (now - self.last_refill).num_milliseconds() as f64 / 1000.0;
        let tokens_to_add = elapsed * self.refill_rate;

        self.tokens = (self.tokens + tokens_to_add).min(self.capacity);
        self.last_refill = now;
    }

    /// Try to consume tokens
    fn consume(&mut self, amount: f64) -> bool {
        self.refill();
        if self.tokens >= amount {
            self.tokens -= amount;
            true
        } else {
            false
        }
    }

    /// Get remaining tokens
    fn remaining(&self) -> f64 {
        self.tokens
    }
}

impl RateLimiter {
    /// Create a new rate limiter
    pub fn new(capacity: u32, refill_rate: f64) -> Self {
        Self {
            buckets: Arc::new(RwLock::new(HashMap::new())),
            capacity,
            refill_rate,
        }
    }

    /// Check if a request is allowed (consumes 1 token)
    pub async fn check(&self, user_id: &UserId) -> RateLimitResult {
        self.check_with_cost(user_id, 1.0).await
    }

    /// Check with custom cost
    pub async fn check_with_cost(&self, user_id: &UserId, cost: f64) -> RateLimitResult {
        let mut buckets = self.buckets.write().await;
        let bucket = buckets
            .entry(user_id.clone())
            .or_insert_with(|| TokenBucket::new(self.capacity as f64, self.refill_rate));

        if bucket.consume(cost) {
            RateLimitResult::Allowed {
                remaining: bucket.remaining() as u32,
                reset_after_secs: ((self.capacity as f64 - bucket.remaining()) / self.refill_rate)
                    as u64,
            }
        } else {
            RateLimitResult::Denied {
                retry_after_secs: ((cost - bucket.remaining()) / self.refill_rate) as u64,
            }
        }
    }

    /// Get current bucket state for a user
    pub async fn get_state(&self, user_id: &UserId) -> Option<RateLimitState> {
        let buckets = self.buckets.read().await;
        buckets.get(user_id).map(|b| RateLimitState {
            remaining: b.remaining() as u32,
            capacity: self.capacity,
        })
    }
}

/// Rate limit check result
#[derive(Debug, Clone)]
pub enum RateLimitResult {
    /// Request is allowed
    Allowed {
        /// Remaining tokens
        remaining: u32,
        /// Seconds until bucket is full
        reset_after_secs: u64,
    },
    /// Request is denied
    Denied {
        /// Seconds until request can be retried
        retry_after_secs: u64,
    },
}

impl RateLimitResult {
    /// Check if the request is allowed
    pub fn is_allowed(&self) -> bool {
        matches!(self, RateLimitResult::Allowed { .. })
    }
}

/// Rate limit state
#[derive(Debug, Clone)]
pub struct RateLimitState {
    /// Remaining tokens
    pub remaining: u32,
    /// Bucket capacity
    pub capacity: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_allowlist() {
        let allowlist = Allowlist::new();
        let user_id = UserId::new("user1");

        assert!(!allowlist.is_allowed(&user_id).await);

        allowlist.allow_user(user_id.clone(), None, None, None).await;
        assert!(allowlist.is_allowed(&user_id).await);

        allowlist.deny_user(&user_id).await;
        assert!(!allowlist.is_allowed(&user_id).await);
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let limiter = RateLimiter::new(10, 1.0); // 10 tokens, 1 per second
        let user_id = UserId::new("user1");

        // Should allow first 10 requests
        for _ in 0..10 {
            assert!(limiter.check(&user_id).await.is_allowed());
        }

        // 11th request should be denied
        assert!(!limiter.check(&user_id).await.is_allowed());
    }

    #[test]
    fn test_token_bucket() {
        let mut bucket = TokenBucket::new(10.0, 1.0);
        assert!(bucket.consume(5.0));
        assert_eq!(bucket.remaining(), 5.0);

        assert!(bucket.consume(5.0));
        assert_eq!(bucket.remaining(), 0.0);

        assert!(!bucket.consume(1.0));
    }

    #[tokio::test]
    async fn test_auth_manager() {
        let auth = AuthManager::new();
        let user = User::new("user1", "Test User");

        assert!(!auth.user_exists(&user.id).await);

        auth.register_user(user.clone()).await.unwrap();
        assert!(auth.user_exists(&user.id).await);

        let session = auth.create_session(user.id.clone(), 24).await.unwrap();
        assert!(auth.validate_session(&session.token).await.is_some());
    }
}
