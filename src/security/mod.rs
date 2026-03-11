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

/// Rate limit headers for HTTP responses
#[derive(Debug, Clone)]
pub struct RateLimitHeaders {
    /// The maximum number of requests allowed in the current window
    pub limit: u32,
    /// The number of requests remaining in the current window
    pub remaining: u32,
    /// Unix timestamp when the rate limit resets
    pub reset: u64,
    /// Seconds until the rate limit resets (optional, for convenience)
    pub reset_after: Option<u64>,
    /// The rate limit policy (e.g., "10;w=60" for 10 requests per 60 seconds)
    pub policy: String,
}

impl RateLimitHeaders {
    /// Create headers from a rate limit result
    pub fn from_result(result: &RateLimitResult, capacity: u32, policy: impl Into<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        match result {
            RateLimitResult::Allowed {
                remaining,
                reset_after_secs,
            } => Self {
                limit: capacity,
                remaining: *remaining,
                reset: now + reset_after_secs,
                reset_after: Some(*reset_after_secs),
                policy: policy.into(),
            },
            RateLimitResult::Denied { retry_after_secs } => Self {
                limit: capacity,
                remaining: 0,
                reset: now + retry_after_secs,
                reset_after: Some(*retry_after_secs),
                policy: policy.into(),
            },
        }
    }

    /// Convert to HTTP header tuples
    pub fn to_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            ("X-RateLimit-Limit".to_string(), self.limit.to_string()),
            ("X-RateLimit-Remaining".to_string(), self.remaining.to_string()),
            ("X-RateLimit-Reset".to_string(), self.reset.to_string()),
            ("RateLimit-Policy".to_string(), self.policy.clone()),
        ];

        if let Some(reset_after) = self.reset_after {
            headers.push(("Retry-After".to_string(), reset_after.to_string()));
            headers.push(("X-RateLimit-Reset-After".to_string(), reset_after.to_string()));
        }

        headers
    }

    /// Create headers for a successful request with remaining quota
    pub fn allowed(remaining: u32, reset: u64, policy: impl Into<String>) -> Self {
        Self {
            limit: remaining + 1,
            remaining,
            reset,
            reset_after: None,
            policy: policy.into(),
        }
    }

    /// Create headers for a rate-limited request
    pub fn denied(retry_after: u64, policy: impl Into<String>) -> Self {
        Self {
            limit: 0,
            remaining: 0,
            reset: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                + retry_after,
            reset_after: Some(retry_after),
            policy: policy.into(),
        }
    }
}

/// Rate limit notification for users
#[derive(Debug, Clone)]
pub struct RateLimitNotification {
    /// Whether the request was allowed
    pub allowed: bool,
    /// Remaining requests
    pub remaining: u32,
    /// Total capacity
    pub limit: u32,
    /// Reset timestamp
    pub reset_at: chrono::DateTime<chrono::Utc>,
    /// Human-readable message
    pub message: String,
}

impl RateLimitNotification {
    /// Create a notification from rate limit headers
    pub fn from_headers(headers: &RateLimitHeaders) -> Self {
        let reset_at = chrono::DateTime::from_timestamp(headers.reset as i64, 0)
            .unwrap_or_else(chrono::Utc::now);

        let (allowed, message) = if headers.remaining == 0 {
            (
                false,
                format!(
                    "Rate limit exceeded. Please try again in {} seconds.",
                    headers.reset_after.unwrap_or(60)
                ),
            )
        } else {
            let percentage = (headers.remaining as f32 / headers.limit as f32 * 100.0) as u32;

            let msg = if percentage < 20 {
                format!(
                    "Warning: You have {} requests remaining ({}% of your quota).",
                    headers.remaining, percentage
                )
            } else {
                format!(
                    "{} of {} requests remaining.",
                    headers.remaining, headers.limit
                )
            };

            (true, msg)
        };

        Self {
            allowed,
            remaining: headers.remaining,
            limit: headers.limit,
            reset_at,
            message,
        }
    }

    /// Create a simple notification
    pub fn simple(remaining: u32, limit: u32) -> Self {
        Self {
            allowed: remaining > 0,
            remaining,
            limit,
            reset_at: chrono::Utc::now() + chrono::Duration::minutes(1),
            message: format!("{} of {} requests remaining.", remaining, limit),
        }
    }

    /// Format as a user-friendly message
    pub fn to_message(&self) -> String {
        self.message.clone()
    }
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

/// HTTP Security Headers
///
/// Provides standard security headers for HTTP responses
pub mod headers {
    use std::collections::HashMap;

    /// Security header configuration
    #[derive(Debug, Clone)]
    pub struct SecurityHeaders {
        headers: HashMap<String, String>,
    }

    impl Default for SecurityHeaders {
        fn default() -> Self {
            Self::secure()
        }
    }

    impl SecurityHeaders {
        /// Create a secure default configuration
        pub fn secure() -> Self {
            let mut headers = HashMap::new();

            // Content Security Policy
            headers.insert(
                "Content-Security-Policy".to_string(),
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline';".to_string(),
            );

            // X-Content-Type-Options
            headers.insert("X-Content-Type-Options".to_string(), "nosniff".to_string());

            // X-Frame-Options
            headers.insert("X-Frame-Options".to_string(), "DENY".to_string());

            // X-XSS-Protection
            headers.insert("X-XSS-Protection".to_string(), "1; mode=block".to_string());

            // Referrer-Policy
            headers.insert("Referrer-Policy".to_string(), "strict-origin-when-cross-origin".to_string());

            // Permissions-Policy
            headers.insert(
                "Permissions-Policy".to_string(),
                "camera=(), microphone=(), geolocation=()".to_string(),
            );

            // Strict-Transport-Security (HSTS)
            headers.insert(
                "Strict-Transport-Security".to_string(),
                "max-age=31536000; includeSubDomains".to_string(),
            );

            Self { headers }
        }

        /// Create an empty configuration (no security headers)
        pub fn empty() -> Self {
            Self {
                headers: HashMap::new(),
            }
        }

        /// Add a custom header
        pub fn add(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
            self.headers.insert(name.into(), value.into());
            self
        }

        /// Remove a header
        pub fn remove(mut self, name: &str) -> Self {
            self.headers.remove(name);
            self
        }

        /// Get all headers as a HashMap
        pub fn headers(&self) -> &HashMap<String, String> {
            &self.headers
        }

        /// Convert to a Vec of tuples for HTTP frameworks
        pub fn to_vec(&self) -> Vec<(String, String)> {
            self.headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        }

        /// Apply CORS headers
        pub fn with_cors(mut self, allowed_origin: impl Into<String>) -> Self {
            self.headers.insert(
                "Access-Control-Allow-Origin".to_string(),
                allowed_origin.into(),
            );
            self.headers.insert(
                "Access-Control-Allow-Methods".to_string(),
                "GET, POST, PUT, DELETE, OPTIONS".to_string(),
            );
            self.headers.insert(
                "Access-Control-Allow-Headers".to_string(),
                "Content-Type, Authorization".to_string(),
            );
            self
        }

        /// Set Content-Security-Policy
        pub fn with_csp(mut self, policy: impl Into<String>) -> Self {
            self.headers.insert("Content-Security-Policy".to_string(), policy.into());
            self
        }

        /// API-specific headers (more permissive for API responses)
        pub fn api() -> Self {
            let mut headers = HashMap::new();

            headers.insert("X-Content-Type-Options".to_string(), "nosniff".to_string());
            headers.insert("X-Frame-Options".to_string(), "DENY".to_string());
            headers.insert(
                "Strict-Transport-Security".to_string(),
                "max-age=31536000; includeSubDomains".to_string(),
            );

            Self { headers }
        }
    }

    /// Default secure headers for web applications
    pub fn default_headers() -> SecurityHeaders {
        SecurityHeaders::secure()
    }

    /// API-specific headers
    pub fn api_headers() -> SecurityHeaders {
        SecurityHeaders::api()
    }

    /// CORS headers for API
    pub fn cors_headers(allowed_origin: impl Into<String>) -> SecurityHeaders {
        SecurityHeaders::api().with_cors(allowed_origin)
    }
}

/// Device fingerprinting for security tracking
pub mod fingerprint {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    /// Device fingerprint information
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct DeviceFingerprint {
        /// Raw fingerprint hash
        pub hash: String,
        /// Components that make up the fingerprint
        pub components: FingerprintComponents,
    }

    /// Components used to generate a device fingerprint
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct FingerprintComponents {
        /// User agent string
        pub user_agent: Option<String>,
        /// IP address
        pub ip_address: Option<String>,
        /// Platform/OS
        pub platform: Option<String>,
        /// Screen resolution (for web clients)
        pub screen_resolution: Option<String>,
        /// Timezone
        pub timezone: Option<String>,
        /// Language
        pub language: Option<String>,
        /// Additional custom data
        pub custom_data: Option<String>,
    }

    impl Default for FingerprintComponents {
        fn default() -> Self {
            Self {
                user_agent: None,
                ip_address: None,
                platform: None,
                screen_resolution: None,
                timezone: None,
                language: None,
                custom_data: None,
            }
        }
    }

    impl FingerprintComponents {
        /// Create new empty components
        pub fn new() -> Self {
            Self::default()
        }

        /// Add user agent
        pub fn with_user_agent(mut self, ua: impl Into<String>) -> Self {
            self.user_agent = Some(ua.into());
            self
        }

        /// Add IP address
        pub fn with_ip(mut self, ip: impl Into<String>) -> Self {
            self.ip_address = Some(ip.into());
            self
        }

        /// Add platform
        pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
            self.platform = Some(platform.into());
            self
        }

        /// Add screen resolution
        pub fn with_screen_resolution(mut self, res: impl Into<String>) -> Self {
            self.screen_resolution = Some(res.into());
            self
        }

        /// Add timezone
        pub fn with_timezone(mut self, tz: impl Into<String>) -> Self {
            self.timezone = Some(tz.into());
            self
        }

        /// Add language
        pub fn with_language(mut self, lang: impl Into<String>) -> Self {
            self.language = Some(lang.into());
            self
        }

        /// Add custom data
        pub fn with_custom_data(mut self, data: impl Into<String>) -> Self {
            self.custom_data = Some(data.into());
            self
        }
    }

    impl DeviceFingerprint {
        /// Generate a fingerprint from components
        pub fn from_components(components: FingerprintComponents) -> Self {
            let hash = Self::compute_hash(&components);
            Self { hash, components }
        }

        /// Generate a simple fingerprint from user agent and IP
        pub fn simple(user_agent: impl Into<String>, ip: impl Into<String>) -> Self {
            let components = FingerprintComponents::new()
                .with_user_agent(user_agent)
                .with_ip(ip);
            Self::from_components(components)
        }

        /// Compute hash from components
        fn compute_hash(components: &FingerprintComponents) -> String {
            let mut hasher = DefaultHasher::new();
            components.hash(&mut hasher);
            format!("{:x}", hasher.finish())
        }

        /// Get fingerprint hash
        pub fn hash(&self) -> &str {
            &self.hash
        }

        /// Check if this fingerprint matches another (compares hashes)
        pub fn matches(&self, other: &DeviceFingerprint) -> bool {
            self.hash == other.hash
        }

        /// Check if this fingerprint is similar to another (allows partial matches)
        pub fn is_similar(&self, other: &DeviceFingerprint) -> bool {
            // Exact hash match
            if self.hash == other.hash {
                return true;
            }

            // Check if key components match
            let self_comp = &self.components;
            let other_comp = &other.components;

            // If IP matches exactly, consider it similar
            if self_comp.ip_address.is_some()
                && self_comp.ip_address == other_comp.ip_address
            {
                return true;
            }

            // If multiple components match, consider it similar
            let mut matching_components = 0;
            let mut total_components = 0;

            if self_comp.user_agent.is_some() && other_comp.user_agent.is_some() {
                total_components += 1;
                if self_comp.user_agent == other_comp.user_agent {
                    matching_components += 1;
                }
            }

            if self_comp.platform.is_some() && other_comp.platform.is_some() {
                total_components += 1;
                if self_comp.platform == other_comp.platform {
                    matching_components += 1;
                }
            }

            if self_comp.timezone.is_some() && other_comp.timezone.is_some() {
                total_components += 1;
                if self_comp.timezone == other_comp.timezone {
                    matching_components += 1;
                }
            }

            // Consider similar if at least 2 components match and more than half match
            total_components > 0
                && matching_components >= 2
                && matching_components * 2 >= total_components
        }

        /// Generate a human-readable description
        pub fn description(&self) -> String {
            let comp = &self.components;
            let mut parts = Vec::new();

            if let Some(ref platform) = comp.platform {
                parts.push(platform.clone());
            }

            if let Some(ref ip) = comp.ip_address {
                // Truncate IP for privacy
                let truncated = if ip.contains(':') {
                    "IPv6".to_string()
                } else {
                    ip.split('.').take(2).collect::<Vec<_>>().join(".") + ".xx.xx"
                };
                parts.push(truncated);
            }

            if let Some(ref tz) = comp.timezone {
                parts.push(tz.clone());
            }

            if parts.is_empty() {
                format!("Device {}", self.hash)
            } else {
                format!("{} ({})", parts.join(" / "), &self.hash[..8])
            }
        }
    }

    /// Fingerprint registry for tracking devices
    #[derive(Debug, Default)]
    pub struct FingerprintRegistry {
        fingerprints: std::collections::HashMap<String, Vec<DeviceFingerprint>>,
    }

    impl FingerprintRegistry {
        /// Create a new registry
        pub fn new() -> Self {
            Self::default()
        }

        /// Register a fingerprint for a user
        pub fn register(&mut self, user_id: impl Into<String>, fingerprint: DeviceFingerprint) {
            let user_id = user_id.into();
            let entry = self.fingerprints.entry(user_id).or_default();

            // Check if already exists
            if !entry.iter().any(|f| f.matches(&fingerprint)) {
                entry.push(fingerprint);
            }
        }

        /// Check if a fingerprint is known for a user
        pub fn is_known(&self, user_id: impl AsRef<str>, fingerprint: &DeviceFingerprint) -> bool {
            self.fingerprints
                .get(user_id.as_ref())
                .map(|fingerprints| fingerprints.iter().any(|f| f.matches(fingerprint)))
                .unwrap_or(false)
        }

        /// Check if a fingerprint is similar to known ones
        pub fn is_similar(&self, user_id: impl AsRef<str>, fingerprint: &DeviceFingerprint) -> bool {
            self.fingerprints
                .get(user_id.as_ref())
                .map(|fingerprints| fingerprints.iter().any(|f| f.is_similar(fingerprint)))
                .unwrap_or(false)
        }

        /// Get all fingerprints for a user
        pub fn get_user_fingerprints(&self, user_id: impl AsRef<str>) -> Vec<DeviceFingerprint> {
            self.fingerprints
                .get(user_id.as_ref())
                .cloned()
                .unwrap_or_default()
        }

        /// Remove a user's fingerprints
        pub fn remove_user(&mut self, user_id: impl AsRef<str>) {
            self.fingerprints.remove(user_id.as_ref());
        }

        /// Clear all fingerprints
        pub fn clear(&mut self) {
            self.fingerprints.clear();
        }
    }
}

#[cfg(test)]
mod header_tests {
    use super::headers::*;

    #[test]
    fn test_default_headers() {
        let headers = default_headers();
        assert!(headers.headers().contains_key("Content-Security-Policy"));
        assert!(headers.headers().contains_key("X-Content-Type-Options"));
        assert!(headers.headers().contains_key("X-Frame-Options"));
        assert!(headers.headers().contains_key("Strict-Transport-Security"));
    }

    #[test]
    fn test_api_headers() {
        let headers = api_headers();
        assert!(!headers.headers().contains_key("Content-Security-Policy"));
        assert!(headers.headers().contains_key("X-Content-Type-Options"));
    }

    #[test]
    fn test_with_cors() {
        let headers = api_headers().with_cors("https://example.com");
        assert_eq!(
            headers.headers().get("Access-Control-Allow-Origin"),
            Some(&"https://example.com".to_string())
        );
    }

    #[test]
    fn test_custom_header() {
        let headers = SecurityHeaders::empty().add("X-Custom", "value");
        assert_eq!(headers.headers().get("X-Custom"), Some(&"value".to_string()));
    }
}
