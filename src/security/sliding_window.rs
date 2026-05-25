//! Sliding Window Rate Limiter for Manta
//!
//! Provides per-user, per-endpoint rate limiting using a sliding window algorithm.
//! Unlike token bucket (which allows bursts), sliding window tracks actual
//! request timestamps and enforces strict rate limits over time.

use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
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

/// Request timestamp entry in the sliding window
#[derive(Debug, Clone)]
struct WindowEntry {
    /// When the request was made
    timestamp: Instant,
}

/// Per-key rate limit state
#[derive(Debug)]
struct WindowState {
    /// Request timestamps in the current window
    requests: VecDeque<WindowEntry>,
    /// Window duration
    window_size: Duration,
    /// Maximum requests per window
    max_requests: u32,
}

impl WindowState {
    /// Create a new window state
    fn new(window_size: Duration, max_requests: u32) -> Self {
        Self {
            requests: VecDeque::with_capacity(max_requests as usize),
            window_size,
            max_requests,
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
                self.window_size - (now - front.timestamp)
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
}

impl SlidingWindowRateLimiter {
    /// Create a new sliding window rate limiter
    pub fn new(window_size: Duration, max_requests: u32) -> Self {
        Self {
            windows: Arc::new(DashMap::new()),
            default_window_size: window_size,
            default_max_requests: max_requests,
        }
    }

    /// Check if a request is allowed without recording it
    pub fn check(&self, key: &RateLimitKey) -> RateLimitResult {
        let now = Instant::now();

        let mut entry = self.windows.entry(key.clone()).or_insert_with(|| {
            WindowState::new(self.default_window_size, self.default_max_requests)
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
        let now = Instant::now();

        let mut entry = self.windows.entry(key.clone()).or_insert_with(|| {
            WindowState::new(self.default_window_size, self.default_max_requests)
        });

        let allowed = entry.check(now);
        let remaining = entry.remaining(now);
        let reset_after = entry.reset_after(now);

        if allowed {
            entry.record(now);
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

    /// Reset rate limit for a key (useful for admin operations)
    pub fn reset(&self, key: &RateLimitKey) {
        self.windows.remove(key);
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

        for key in keys_to_remove {
            self.windows.remove(&key);
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

        if removed > 0 {
            debug!(removed = removed, "Cleaned up old rate limit windows");
        }
    }

    /// Get total number of tracked windows
    pub fn window_count(&self) -> usize {
        self.windows.len()
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
#[derive(Debug, Clone)]
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

/// Axum middleware layer for sliding window rate limiting
#[allow(unexpected_cfgs)]
#[cfg(feature = "web")]
pub mod middleware {
    use super::*;
    use axum::{
        body::Body,
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
}
