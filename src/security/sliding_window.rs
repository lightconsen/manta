//! Sliding Window Rate Limiter for Syscity
//!
//! Provides per-user, per-endpoint rate limiting using a sliding window algorithm.
//! Unlike token bucket (which allows bursts), sliding window tracks actual
//! request timestamps and enforces strict rate limits over time.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// Unique key for rate limiting: (user_id, endpoint)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateLimitKey {
    pub user_id: String,
    pub endpoint: String,
}

impl RateLimitKey {
    /// Create a new rate limit key
    pub fn new(user_id: impl Into<String>, endpoint: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            endpoint: endpoint.into(),
        }
    }
}

/// Lockout configuration for a sliding window tier.
///
/// After `max_failures` denied attempts within `window_size`, the key is
/// locked out for `lockout_duration`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockoutConfig {
    /// Enable lockout tracking.
    pub enabled: bool,
    /// Number of failures within the window that trigger lockout.
    pub max_failures: u32,
    /// Window in which failures are counted.
    pub window_secs: u64,
    /// How long the key remains locked out.
    pub lockout_secs: u64,
}

impl Default for LockoutConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_failures: 5,
            window_secs: 300,
            lockout_secs: 900,
        }
    }
}

/// A single recorded attempt for a rate-limit key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttemptRecord {
    /// Unix timestamp in seconds.
    pub timestamp_secs: u64,
    /// Whether the attempt was allowed.
    pub allowed: bool,
    /// Optional reason for denial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Persistent snapshot of attempts for a single key.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AttemptLog {
    pub user_id: String,
    pub endpoint: String,
    pub attempts: Vec<AttemptRecord>,
}

/// Per-key lockout state.
#[derive(Debug, Clone)]
struct LockoutState {
    /// Timestamps of recent failures.
    failures: VecDeque<Instant>,
    /// When the key is locked out until, if at all.
    locked_until: Option<Instant>,
    /// Window size for counting failures.
    window_size: Duration,
    /// Maximum failures before lockout.
    max_failures: u32,
    /// Lockout duration.
    lockout_duration: Duration,
}

impl LockoutState {
    fn new(config: &LockoutConfig) -> Self {
        Self {
            failures: VecDeque::new(),
            locked_until: None,
            window_size: Duration::from_secs(config.window_secs),
            max_failures: config.max_failures,
            lockout_duration: Duration::from_secs(config.lockout_secs),
        }
    }

    /// Clean failures older than the window.
    fn clean_old_failures(&mut self, now: Instant) {
        let window_start = now - self.window_size;
        while let Some(front) = self.failures.front() {
            if *front < window_start {
                self.failures.pop_front();
            } else {
                break;
            }
        }
    }

    /// Check if currently locked out.
    fn is_locked_out(&mut self, now: Instant) -> bool {
        if let Some(locked_until) = self.locked_until {
            if now < locked_until {
                return true;
            }
            self.locked_until = None;
        }
        false
    }

    /// Record a failure and return true if a new lockout was triggered.
    fn record_failure(&mut self, now: Instant) -> bool {
        self.clean_old_failures(now);
        self.failures.push_back(now);
        if self.failures.len() >= self.max_failures as usize {
            self.locked_until = Some(now + self.lockout_duration);
            return true;
        }
        false
    }
}

/// Result of a lockout check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockoutCheck {
    /// Key is allowed; no lockout active.
    Allowed,
    /// Key is locked out; retry after the given seconds.
    LockedOut { retry_after_secs: u64 },
}

impl LockoutCheck {
    /// Check if the key is currently locked out.
    pub fn is_locked_out(&self) -> bool {
        matches!(self, LockoutCheck::LockedOut { .. })
    }
}

/// Request timestamp entry in the sliding window
#[derive(Debug, Clone)]
struct WindowEntry {
    /// When the request was made
    timestamp: Instant,
}

#[derive(Debug)]
struct WindowState {
    /// Request timestamps in the current window
    requests: VecDeque<WindowEntry>,
    /// Window duration
    window_size: Duration,
    /// Maximum requests per window
    max_requests: u32,
    /// Recent attempt records for this key.
    attempts: VecDeque<AttemptEntry>,
    /// Maximum attempts to retain for serialization.
    max_attempt_history: usize,
}

/// Internal attempt entry with an `Instant` timestamp.
#[derive(Debug, Clone)]
struct AttemptEntry {
    timestamp: Instant,
    allowed: bool,
    reason: Option<String>,
}

impl AttemptEntry {
    fn to_record(&self, now: Instant) -> Option<AttemptRecord> {
        let elapsed = now.duration_since(self.timestamp).as_secs();
        let system_now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        let timestamp_secs = system_now.saturating_sub(elapsed);
        Some(AttemptRecord {
            timestamp_secs,
            allowed: self.allowed,
            reason: self.reason.clone(),
        })
    }
}

impl WindowState {
    /// Create a new window state
    fn new(window_size: Duration, max_requests: u32, max_attempt_history: usize) -> Self {
        Self {
            requests: VecDeque::with_capacity(max_requests as usize),
            window_size,
            max_requests,
            attempts: VecDeque::new(),
            max_attempt_history,
        }
    }

    /// Clean old entries outside the window
    fn clean_old_entries(&mut self, now: Instant) {
        let window_start = now - self.window_size;
        while let Some(front) = self.requests.front() {
            if front.timestamp < window_start {
                self.requests.pop_front();
            } else {
                break;
            }
        }
    }

    /// Check if a request is allowed (doesn't record it)
    fn check(&mut self, now: Instant) -> bool {
        self.clean_old_entries(now);
        self.requests.len() < self.max_requests as usize
    }

    /// Record a request
    fn record(&mut self, now: Instant) {
        self.requests.push_back(WindowEntry { timestamp: now });
    }

    /// Get current request count
    fn count(&mut self, now: Instant) -> usize {
        self.clean_old_entries(now);
        self.requests.len()
    }

    /// Get remaining requests
    fn remaining(&mut self, now: Instant) -> u32 {
        self.clean_old_entries(now);
        self.max_requests.saturating_sub(self.requests.len() as u32)
    }

    /// Get time until oldest request expires
    fn reset_after(&self, now: Instant) -> Duration {
        if let Some(front) = self.requests.front() {
            let window_start = now - self.window_size;
            if front.timestamp > window_start {
                let elapsed = now.duration_since(front.timestamp);
                self.window_size.saturating_sub(elapsed)
            } else {
                Duration::ZERO
            }
        } else {
            Duration::ZERO
        }
    }
}

/// Sliding window rate limiter
///
/// Tracks request timestamps per (user_id, endpoint) pair and enforces
/// rate limits based on actual request times within a sliding window.
#[derive(Debug, Clone)]
pub struct SlidingWindowRateLimiter {
    /// Windows per key
    windows: Arc<DashMap<RateLimitKey, WindowState>>,
    /// Default window size
    default_window_size: Duration,
    /// Default max requests per window
    default_max_requests: u32,
    /// Per-key lockout state.
    lockouts: Arc<DashMap<RateLimitKey, LockoutState>>,
    /// Lockout configuration.
    lockout_config: Option<LockoutConfig>,
    /// Maximum attempt history retained per key.
    max_attempt_history: usize,
}

impl SlidingWindowRateLimiter {
    /// Create a new sliding window rate limiter
    pub fn new(window_size: Duration, max_requests: u32) -> Self {
        Self {
            windows: Arc::new(DashMap::new()),
            default_window_size: window_size,
            default_max_requests: max_requests,
            lockouts: Arc::new(DashMap::new()),
            lockout_config: None,
            max_attempt_history: 1000,
        }
    }

    /// Create a new limiter with lockout configuration.
    pub fn with_lockout(
        window_size: Duration,
        max_requests: u32,
        lockout_config: LockoutConfig,
    ) -> Self {
        Self {
            windows: Arc::new(DashMap::new()),
            default_window_size: window_size,
            default_max_requests: max_requests,
            lockouts: Arc::new(DashMap::new()),
            lockout_config: Some(lockout_config),
            max_attempt_history: 1000,
        }
    }

    /// Check if a key is currently locked out.
    pub fn check_lockout(&self, key: &RateLimitKey) -> LockoutCheck {
        let Some(config) = self.lockout_config.as_ref().filter(|c| c.enabled) else {
            return LockoutCheck::Allowed;
        };

        let now = Instant::now();
        let mut entry = self
            .lockouts
            .entry(key.clone())
            .or_insert_with(|| LockoutState::new(config));

        if entry.is_locked_out(now) {
            let retry_after_secs = entry
                .locked_until
                .map(|until| until.duration_since(now).as_secs().max(1))
                .unwrap_or(config.lockout_secs);
            return LockoutCheck::LockedOut { retry_after_secs };
        }

        LockoutCheck::Allowed
    }

    /// Record an attempt (success or failure) for a key.
    ///
    /// This is used both for attempt serialization and for lockout tracking.
    /// Returns `true` if the failure triggered a new lockout.
    pub fn record_attempt(
        &self,
        key: &RateLimitKey,
        allowed: bool,
        reason: Option<String>,
    ) -> bool {
        let now = Instant::now();

        // Record in the window state for serialization.
        if let Some(mut entry) = self.windows.get_mut(key) {
            entry.attempts.push_back(AttemptEntry {
                timestamp: now,
                allowed,
                reason,
            });
            while entry.attempts.len() > entry.max_attempt_history {
                entry.attempts.pop_front();
            }
        }

        // Update lockout state on failure.
        if !allowed {
            if let Some(config) = self.lockout_config.as_ref().filter(|c| c.enabled) {
                let mut entry = self
                    .lockouts
                    .entry(key.clone())
                    .or_insert_with(|| LockoutState::new(config));
                return entry.record_failure(now);
            }
        }
        false
    }

    /// Check if a request is allowed without recording it
    pub fn check(&self, key: &RateLimitKey) -> RateLimitResult {
        if let LockoutCheck::LockedOut { retry_after_secs } = self.check_lockout(key) {
            return RateLimitResult::Denied { retry_after_secs };
        }

        let now = Instant::now();

        let mut entry = self.windows.entry(key.clone()).or_insert_with(|| {
            WindowState::new(
                self.default_window_size,
                self.default_max_requests,
                self.max_attempt_history,
            )
        });

        let allowed = entry.check(now);
        let remaining = entry.remaining(now);
        let reset_after = entry.reset_after(now);

        if allowed {
            RateLimitResult::Allowed {
                remaining,
                reset_after_secs: reset_after.as_secs(),
            }
        } else {
            RateLimitResult::Denied {
                retry_after_secs: reset_after.as_secs().max(1),
            }
        }
    }

    /// Check and record a request
    pub fn check_and_record(&self, key: &RateLimitKey) -> RateLimitResult {
        if let LockoutCheck::LockedOut { retry_after_secs } = self.check_lockout(key) {
            self.record_attempt(key, false, Some("locked_out".to_string()));
            return RateLimitResult::Denied { retry_after_secs };
        }

        let now = Instant::now();

        // Compute the result while holding the window entry, then drop the
        // mutable reference before recording the attempt. Holding the entry
        // across `record_attempt` would deadlock because `record_attempt`
        // also needs a mutable reference to the same window state.
        let (allowed, remaining, reset_after) = {
            let mut entry = self.windows.entry(key.clone()).or_insert_with(|| {
                WindowState::new(
                    self.default_window_size,
                    self.default_max_requests,
                    self.max_attempt_history,
                )
            });

            let allowed = entry.check(now);
            let remaining = entry.remaining(now);
            let reset_after = entry.reset_after(now);

            if allowed {
                entry.record(now);
            }

            (allowed, remaining, reset_after)
        };

        if allowed {
            self.record_attempt(key, true, None);
            debug!(
                user_id = %key.user_id,
                endpoint = %key.endpoint,
                remaining = remaining,
                "Rate limit check passed"
            );
            RateLimitResult::Allowed {
                remaining: remaining.saturating_sub(1),
                reset_after_secs: reset_after.as_secs(),
            }
        } else {
            self.record_attempt(key, false, Some("rate_limited".to_string()));
            warn!(
                user_id = %key.user_id,
                endpoint = %key.endpoint,
                limit = self.default_max_requests,
                window_secs = self.default_window_size.as_secs(),
                "Rate limit exceeded"
            );
            RateLimitResult::Denied {
                retry_after_secs: reset_after.as_secs().max(1),
            }
        }
    }

    /// Get current state for a key
    pub fn get_state(&self, key: &RateLimitKey) -> Option<SlidingWindowState> {
        let now = Instant::now();
        self.windows
            .get_mut(key)
            .map(|mut entry| SlidingWindowState {
                window_size: entry.window_size,
                max_requests: entry.max_requests,
                current_requests: entry.count(now) as u32,
                remaining: entry.remaining(now),
            })
    }

    /// Get lockout state for a key.
    pub fn get_lockout_state(&self, key: &RateLimitKey) -> Option<LockoutStateSnapshot> {
        let now = Instant::now();
        let mut entry = self.lockouts.get_mut(key)?;
        entry.clean_old_failures(now);
        Some(LockoutStateSnapshot {
            failure_count: entry.failures.len() as u32,
            locked: entry.is_locked_out(now),
            retry_after_secs: entry
                .locked_until
                .map(|until| until.duration_since(now).as_secs()),
        })
    }

    /// Reset rate limit for a key (useful for admin operations)
    pub fn reset(&self, key: &RateLimitKey) {
        self.windows.remove(key);
        self.lockouts.remove(key);
        debug!(user_id = %key.user_id, endpoint = %key.endpoint, "Rate limit reset");
    }

    /// Reset all rate limits for a user
    pub fn reset_user(&self, user_id: &str) {
        let keys_to_remove: Vec<_> = self
            .windows
            .iter()
            .filter(|entry| entry.key().user_id == user_id)
            .map(|entry| entry.key().clone())
            .collect();

        for key in &keys_to_remove {
            self.windows.remove(key);
        }

        let lockout_keys_to_remove: Vec<_> = self
            .lockouts
            .iter()
            .filter(|entry| entry.key().user_id == user_id)
            .map(|entry| entry.key().clone())
            .collect();

        for key in &lockout_keys_to_remove {
            self.lockouts.remove(key);
        }

        debug!(user_id = %user_id, "All rate limits reset for user");
    }

    /// Clean up old entries (call periodically to prevent memory growth)
    pub fn cleanup(&self) {
        let now = Instant::now();
        let mut removed = 0;

        self.windows.retain(|_, state| {
            let has_recent = state
                .requests
                .back()
                .map(|e| now.duration_since(e.timestamp) < state.window_size * 2)
                .unwrap_or(false);
            if !has_recent {
                removed += 1;
            }
            has_recent
        });

        self.lockouts.retain(|_, state| {
            state.clean_old_failures(now);
            let still_locked = state.is_locked_out(now);
            let has_recent_failures = state
                .failures
                .back()
                .map(|e| now.duration_since(*e) < state.window_size * 2)
                .unwrap_or(false);
            still_locked || has_recent_failures
        });

        if removed > 0 {
            debug!(removed = removed, "Cleaned up old rate limit windows");
        }
    }

    /// Serialize all recent attempts to a JSON byte vector.
    ///
    /// The output is a JSON array of `AttemptLog` objects, one per tracked key.
    pub fn serialize_attempts(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&self.attempt_logs())
    }

    /// Return all recent attempts as `AttemptLog` records.
    pub fn attempt_logs(&self) -> Vec<AttemptLog> {
        let now = Instant::now();
        self.windows
            .iter()
            .filter_map(|entry| {
                let key = entry.key();
                let state = entry.value();
                let attempts: Vec<_> = state
                    .attempts
                    .iter()
                    .filter_map(|a| a.to_record(now))
                    .collect();
                if attempts.is_empty() {
                    return None;
                }
                Some(AttemptLog {
                    user_id: key.user_id.clone(),
                    endpoint: key.endpoint.clone(),
                    attempts,
                })
            })
            .collect()
    }

    /// Load attempts from a serialized snapshot.
    ///
    /// This replaces the in-memory attempt history for keys that appear in the
    /// snapshot. It does not affect the request-count windows.
    pub fn load_attempts(&self, data: &[u8]) -> Result<usize, serde_json::Error> {
        let logs: Vec<AttemptLog> = serde_json::from_slice(data)?;
        let now = Instant::now();
        let mut loaded = 0;
        for log in logs {
            let key = RateLimitKey::new(log.user_id, log.endpoint);
            let mut entry = self.windows.entry(key).or_insert_with(|| {
                WindowState::new(
                    self.default_window_size,
                    self.default_max_requests,
                    self.max_attempt_history,
                )
            });
            entry.attempts.clear();
            for record in log.attempts {
                // Convert absolute timestamp to an approximate `Instant` by
                // treating it as `now - age`. This is best-effort; monotonic
                // clocks cannot be reconstructed precisely.
                let system_now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let age_secs = system_now.saturating_sub(record.timestamp_secs);
                let timestamp = now - Duration::from_secs(age_secs.min(u64::MAX / 2));
                entry.attempts.push_back(AttemptEntry {
                    timestamp,
                    allowed: record.allowed,
                    reason: record.reason,
                });
                loaded += 1;
            }
            while entry.attempts.len() > entry.max_attempt_history {
                entry.attempts.pop_front();
            }
        }
        Ok(loaded)
    }

    /// Get total number of tracked windows
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Get total number of tracked lockout states.
    pub fn lockout_count(&self) -> usize {
        self.lockouts.len()
    }

    /// Get default window size
    pub fn window_size(&self) -> Duration {
        self.default_window_size
    }

    /// Get default max requests
    pub fn max_requests(&self) -> u32 {
        self.default_max_requests
    }
}

impl Default for SlidingWindowRateLimiter {
    fn default() -> Self {
        Self::new(Duration::from_secs(60), 100)
    }
}

/// Rate limit check result
#[derive(Debug, Clone)]
pub enum RateLimitResult {
    /// Request is allowed
    Allowed {
        /// Remaining requests in current window
        remaining: u32,
        /// Seconds until the window fully resets
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

    /// Get remaining requests (if allowed)
    pub fn remaining(&self) -> Option<u32> {
        match self {
            RateLimitResult::Allowed { remaining, .. } => Some(*remaining),
            _ => None,
        }
    }

    /// Get retry after seconds (if denied)
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            RateLimitResult::Denied { retry_after_secs } => Some(*retry_after_secs),
            _ => None,
        }
    }
}

/// Current state of a sliding window
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlidingWindowState {
    /// Window size
    pub window_size: Duration,
    /// Maximum requests per window
    pub max_requests: u32,
    /// Current request count
    pub current_requests: u32,
    /// Remaining requests
    pub remaining: u32,
}

/// Snapshot of lockout state for a single key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockoutStateSnapshot {
    /// Number of failures in the current failure window.
    pub failure_count: u32,
    /// Whether the key is currently locked out.
    pub locked: bool,
    /// Seconds until the lockout expires, if locked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

/// Axum middleware layer for sliding window rate limiting
#[allow(unexpected_cfgs)]
#[cfg(feature = "web")]
pub mod middleware {
    use super::*;
    use axum::{
        extract::{ConnectInfo, Request, State},
        http::StatusCode,
        middleware::Next,
        response::{IntoResponse, Response},
        Json,
    };
    use serde_json::json;
    use std::net::SocketAddr;

    /// Extract user ID from request (customize based on your auth)
    pub fn extract_user_id(req: &Request) -> String {
        // Try to get from header first
        if let Some(header) = req.headers().get("x-user-id") {
            if let Ok(value) = header.to_str() {
                return value.to_string();
            }
        }

        // Fall back to IP-based limiting
        if let Some(ConnectInfo(addr)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
            return addr.ip().to_string();
        }

        // Last resort: use "anonymous"
        "anonymous".to_string()
    }

    /// Extract endpoint from request path
    pub fn extract_endpoint(req: &Request) -> String {
        req.uri().path().to_string()
    }

    /// Rate limiting middleware for Axum
    pub async fn rate_limit_middleware(
        State(limiter): State<Arc<SlidingWindowRateLimiter>>,
        req: Request,
        next: Next,
    ) -> Response {
        let user_id = extract_user_id(&req);
        let endpoint = extract_endpoint(&req);
        let key = RateLimitKey::new(user_id, endpoint);

        match limiter.check_and_record(&key) {
            RateLimitResult::Allowed { remaining, reset_after_secs } => {
                // Add rate limit headers to response
                let mut response = next.run(req).await;

                let headers = response.headers_mut();
                headers.insert("x-ratelimit-limit", limiter.max_requests().into());
                headers.insert("x-ratelimit-remaining", remaining.into());
                headers.insert("x-ratelimit-reset", reset_after_secs.into());

                response
            }
            RateLimitResult::Denied { retry_after_secs } => {
                // Return 429 Too Many Requests
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", retry_after_secs.to_string())],
                    Json(json!({
                        "error": "Rate limit exceeded",
                        "retry_after": retry_after_secs,
                        "limit": limiter.max_requests(),
                        "window_secs": limiter.window_size().as_secs()
                    })),
                )
                    .into_response()
            }
        }
    }

    /// Rate limiting middleware with custom key extraction
    pub async fn rate_limit_middleware_with_extractor<F>(
        State((limiter, extractor)): State<(Arc<SlidingWindowRateLimiter>, F)>,
        req: Request,
        next: Next,
    ) -> Response
    where
        F: Fn(&Request) -> RateLimitKey + Send + Sync + Clone,
    {
        let key = extractor(&req);

        match limiter.check_and_record(&key) {
            RateLimitResult::Allowed { remaining, reset_after_secs } => {
                let mut response = next.run(req).await;

                let headers = response.headers_mut();
                headers.insert("x-ratelimit-limit", limiter.max_requests().into());
                headers.insert("x-ratelimit-remaining", remaining.into());
                headers.insert("x-ratelimit-reset", reset_after_secs.into());

                response
            }
            RateLimitResult::Denied { retry_after_secs } => (
                StatusCode::TOO_MANY_REQUESTS,
                [("retry-after", retry_after_secs.to_string())],
                Json(json!({
                    "error": "Rate limit exceeded",
                    "retry_after": retry_after_secs
                })),
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sliding_window_rate_limiter() {
        let limiter = SlidingWindowRateLimiter::new(Duration::from_secs(60), 5);
        let key = RateLimitKey::new("user1", "/api/test");

        // First 5 requests should be allowed
        for i in 0..5 {
            let result = limiter.check_and_record(&key);
            assert!(result.is_allowed(), "Request {} should be allowed", i + 1);
            assert_eq!(result.remaining(), Some(5 - i - 1));
        }

        // 6th request should be denied
        let result = limiter.check_and_record(&key);
        assert!(!result.is_allowed(), "6th request should be denied");
        assert!(result.retry_after().is_some());
    }

    #[test]
    fn test_check_without_record() {
        let limiter = SlidingWindowRateLimiter::new(Duration::from_secs(60), 3);
        let key = RateLimitKey::new("user1", "/api/test");

        // Check multiple times without recording
        for _ in 0..10 {
            let result = limiter.check(&key);
            assert!(result.is_allowed());
        }

        // Now record 3 requests
        for _ in 0..3 {
            limiter.check_and_record(&key);
        }

        // Should now be denied
        assert!(!limiter.check(&key).is_allowed());
    }

    #[test]
    fn test_reset() {
        let limiter = SlidingWindowRateLimiter::new(Duration::from_secs(60), 2);
        let key = RateLimitKey::new("user1", "/api/test");

        // Use up the limit
        limiter.check_and_record(&key);
        limiter.check_and_record(&key);
        assert!(!limiter.check(&key).is_allowed());

        // Reset and try again
        limiter.reset(&key);
        assert!(limiter.check(&key).is_allowed());
    }

    #[test]
    fn test_per_endpoint_limiting() {
        let limiter = SlidingWindowRateLimiter::new(Duration::from_secs(60), 2);
        let user = "user1";

        // Different endpoints have separate windows
        let key1 = RateLimitKey::new(user, "/api/endpoint1");
        let key2 = RateLimitKey::new(user, "/api/endpoint2");

        // Use up limit on endpoint1
        limiter.check_and_record(&key1);
        limiter.check_and_record(&key1);
        assert!(!limiter.check(&key1).is_allowed());

        // But endpoint2 should still work
        assert!(limiter.check(&key2).is_allowed());
    }

    #[test]
    fn test_cleanup() {
        let limiter = SlidingWindowRateLimiter::new(Duration::from_millis(100), 10);
        let key = RateLimitKey::new("user1", "/api/test");

        // Add some requests
        for _ in 0..5 {
            limiter.check_and_record(&key);
        }

        assert_eq!(limiter.window_count(), 1);

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(250));

        // Cleanup should remove old windows
        limiter.cleanup();
        assert_eq!(limiter.window_count(), 0);
    }

    #[test]
    fn test_default_trait_does_not_stack_overflow() {
        // Before the fix, `impl Default` called `Self::default()` recursively,
        // causing a stack overflow. This test verifies the fix works.
        let limiter: SlidingWindowRateLimiter = Default::default();
        assert_eq!(limiter.max_requests(), 100);
        assert_eq!(limiter.window_size(), Duration::from_secs(60));

        // Verify it actually limits
        let key = RateLimitKey::new("user_default", "/api/test");
        for _ in 0..100 {
            assert!(limiter.check_and_record(&key).is_allowed());
        }
        assert!(!limiter.check_and_record(&key).is_allowed());
    }

    #[test]
    fn test_remaining_and_retry_after_helpers() {
        let limiter = SlidingWindowRateLimiter::new(Duration::from_secs(60), 3);
        let key = RateLimitKey::new("user2", "/api/test");

        let r1 = limiter.check_and_record(&key);
        assert!(r1.is_allowed());
        assert_eq!(r1.remaining(), Some(2));

        let r2 = limiter.check_and_record(&key);
        assert_eq!(r2.remaining(), Some(1));

        let r3 = limiter.check_and_record(&key);
        assert_eq!(r3.remaining(), Some(0));

        let denied = limiter.check_and_record(&key);
        assert!(!denied.is_allowed());
        assert_eq!(denied.remaining(), None);
        assert!(denied.retry_after().is_some());
    }

    #[test]
    fn test_lockout_triggers_after_failures() {
        let lockout_config = LockoutConfig {
            enabled: true,
            max_failures: 3,
            window_secs: 60,
            lockout_secs: 1,
        };
        let limiter =
            SlidingWindowRateLimiter::with_lockout(Duration::from_secs(60), 100, lockout_config);
        let key = RateLimitKey::new("user1", "/api/test");

        // First two failures are recorded but do not lock out.
        limiter.record_attempt(&key, false, Some("bad_token".to_string()));
        limiter.record_attempt(&key, false, Some("bad_token".to_string()));
        assert!(!limiter.check_lockout(&key).is_locked_out());

        // Third failure triggers lockout.
        limiter.record_attempt(&key, false, Some("bad_token".to_string()));
        assert!(limiter.check_lockout(&key).is_locked_out());

        // Allowed checks are also denied while locked out.
        let result = limiter.check(&key);
        assert!(!result.is_allowed());

        // Wait for lockout to expire.
        std::thread::sleep(Duration::from_millis(1100));
        assert!(!limiter.check_lockout(&key).is_locked_out());
    }

    #[test]
    fn test_attempt_serialization_roundtrip() {
        let limiter = SlidingWindowRateLimiter::new(Duration::from_secs(60), 5);
        let key = RateLimitKey::new("user1", "/api/test");

        limiter.check_and_record(&key);
        limiter.check_and_record(&key);
        limiter.check_and_record(&key);

        let data = limiter.serialize_attempts().unwrap();
        assert!(!data.is_empty());

        let limiter2 = SlidingWindowRateLimiter::new(Duration::from_secs(60), 5);
        let loaded = limiter2.load_attempts(&data).unwrap();
        assert_eq!(loaded, 3);

        // Loading attempts creates a window entry for the key, but does not
        // restore the request-count window (which is intentionally separate).
        assert_eq!(limiter2.window_count(), 1);
    }

    #[test]
    fn test_lockout_config_default() {
        let config = LockoutConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_failures, 5);
        assert_eq!(config.window_secs, 300);
        assert_eq!(config.lockout_secs, 900);
    }
}
