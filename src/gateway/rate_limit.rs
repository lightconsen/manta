//! Multi-tier Rate Limiting for Manta Gateway
//!
//! Provides sophisticated rate limiting with multiple tiers:
//! - Global: overall API rate limit
//! - Per-user: authenticated user limits
//! - Per-IP: IP-based limits for anonymous requests
//! - Per-endpoint: specific endpoint restrictions
//!
//! Mirrors OpenClaw's layered rate limiting architecture.

use crate::gateway::auth::extract_session_cookie;
use crate::gateway::auth::SessionCookieConfig;
use crate::security::sliding_window::{RateLimitKey, SlidingWindowRateLimiter};
use crate::security::{RateLimitResult, RateLimiter, UserId};
use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use crate::gateway::GatewayState;

/// Multi-tier rate limit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiTierRateLimitConfig {
    /// Global rate limit (all requests)
    pub global: TierConfig,
    /// Per-authenticated-user rate limit
    pub per_user: TierConfig,
    /// Per-IP rate limit (for anonymous requests)
    pub per_ip: TierConfig,
    /// Per-endpoint rate limit
    pub per_endpoint: TierConfig,
}

impl Default for MultiTierRateLimitConfig {
    fn default() -> Self {
        Self {
            global: TierConfig {
                enabled: true,
                capacity: 1000,
                window_secs: 60,
            },
            per_user: TierConfig {
                enabled: true,
                capacity: 100,
                window_secs: 60,
            },
            per_ip: TierConfig {
                enabled: true,
                capacity: 30,
                window_secs: 60,
            },
            per_endpoint: TierConfig {
                enabled: false,
                capacity: 50,
                window_secs: 60,
            },
        }
    }
}

/// Single tier configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// Enable this tier
    pub enabled: bool,
    /// Maximum requests per window
    pub capacity: u32,
    /// Window size in seconds
    pub window_secs: u64,
}

/// Multi-tier rate limiter combining token bucket and sliding window
#[derive(Debug, Clone)]
pub struct MultiTierRateLimiter {
    /// Global sliding window limiter
    global: Arc<SlidingWindowRateLimiter>,
    /// Per-user sliding window limiter
    per_user: Arc<SlidingWindowRateLimiter>,
    /// Per-IP sliding window limiter
    per_ip: Arc<SlidingWindowRateLimiter>,
    /// Per-endpoint sliding window limiter
    per_endpoint: Arc<SlidingWindowRateLimiter>,
    /// Legacy token bucket rate limiter (for backward compat)
    token_bucket: Arc<RateLimiter>,
    /// Configuration
    config: MultiTierRateLimitConfig,
}

impl MultiTierRateLimiter {
    /// Create a new multi-tier rate limiter
    pub fn new(config: MultiTierRateLimitConfig) -> Self {
        Self {
            global: Arc::new(SlidingWindowRateLimiter::new(
                Duration::from_secs(config.global.window_secs),
                config.global.capacity,
            )),
            per_user: Arc::new(SlidingWindowRateLimiter::new(
                Duration::from_secs(config.per_user.window_secs),
                config.per_user.capacity,
            )),
            per_ip: Arc::new(SlidingWindowRateLimiter::new(
                Duration::from_secs(config.per_ip.window_secs),
                config.per_ip.capacity,
            )),
            per_endpoint: Arc::new(SlidingWindowRateLimiter::new(
                Duration::from_secs(config.per_endpoint.window_secs),
                config.per_endpoint.capacity,
            )),
            token_bucket: Arc::new(RateLimiter::new(100, 10.0)),
            config,
        }
    }

    /// Check all tiers for a request
    pub async fn check(
        &self,
        user_id: &UserId,
        ip: Option<std::net::IpAddr>,
        endpoint: &str,
    ) -> MultiTierResult {
        // Check global tier
        if self.config.global.enabled {
            let global_key = RateLimitKey::new("global", endpoint);
            let result = self.global.check_and_record(&global_key);
            if !result.is_allowed() {
                return MultiTierResult::Denied {
                    tier: "global",
                    retry_after_secs: result.retry_after().unwrap_or(60),
                };
            }
        }

        // Check per-user tier (for authenticated users)
        if self.config.per_user.enabled && !user_id.0.starts_with("ip:") && user_id.0 != "anonymous"
        {
            let user_key = RateLimitKey::new(&user_id.0, endpoint);
            let result = self.per_user.check_and_record(&user_key);
            if !result.is_allowed() {
                return MultiTierResult::Denied {
                    tier: "per_user",
                    retry_after_secs: result.retry_after().unwrap_or(60),
                };
            }
        }

        // Check per-IP tier (for anonymous or as fallback)
        if self.config.per_ip.enabled {
            let ip_str = ip
                .map(|i| i.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let ip_key = RateLimitKey::new(format!("ip:{}", ip_str), endpoint);
            let result = self.per_ip.check_and_record(&ip_key);
            if !result.is_allowed() {
                return MultiTierResult::Denied {
                    tier: "per_ip",
                    retry_after_secs: result.retry_after().unwrap_or(60),
                };
            }
        }

        // Check per-endpoint tier
        if self.config.per_endpoint.enabled {
            let endpoint_key = RateLimitKey::new("endpoint", endpoint);
            let result = self.per_endpoint.check_and_record(&endpoint_key);
            if !result.is_allowed() {
                return MultiTierResult::Denied {
                    tier: "per_endpoint",
                    retry_after_secs: result.retry_after().unwrap_or(60),
                };
            }
        }

        // Legacy token bucket check for backward compatibility
        let legacy = self.token_bucket.check(user_id).await;
        if !legacy.is_allowed() {
            return MultiTierResult::Denied {
                tier: "legacy",
                retry_after_secs: match legacy {
                    RateLimitResult::Denied { retry_after_secs } => retry_after_secs,
                    _ => 60,
                },
            };
        }

        let remaining = match legacy {
            RateLimitResult::Allowed { remaining, .. } => remaining,
            _ => 0,
        };

        MultiTierResult::Allowed { remaining }
    }

    /// Get current state summary
    pub fn stats(&self) -> RateLimitStats {
        RateLimitStats {
            global_windows: self.global.window_count(),
            user_windows: self.per_user.window_count(),
            ip_windows: self.per_ip.window_count(),
            endpoint_windows: self.per_endpoint.window_count(),
        }
    }

    /// Cleanup old windows (call periodically)
    pub fn cleanup(&self) {
        self.global.cleanup();
        self.per_user.cleanup();
        self.per_ip.cleanup();
        self.per_endpoint.cleanup();
    }
}

impl Default for MultiTierRateLimiter {
    fn default() -> Self {
        Self::new(MultiTierRateLimitConfig::default())
    }
}

/// Result of multi-tier rate limit check
#[derive(Debug, Clone)]
pub enum MultiTierResult {
    /// Request allowed
    Allowed { remaining: u32 },
    /// Request denied by a specific tier
    Denied {
        tier: &'static str,
        retry_after_secs: u64,
    },
}

impl MultiTierResult {
    /// Check if request is allowed
    pub fn is_allowed(&self) -> bool {
        matches!(self, MultiTierResult::Allowed { .. })
    }

    /// Get retry after seconds
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            MultiTierResult::Denied { retry_after_secs, .. } => Some(*retry_after_secs),
            _ => None,
        }
    }
}

/// Rate limit statistics
#[derive(Debug, Clone, Serialize)]
pub struct RateLimitStats {
    pub global_windows: usize,
    pub user_windows: usize,
    pub ip_windows: usize,
    pub endpoint_windows: usize,
}

/// Extract client IP from request (re-export from middleware for convenience)
fn extract_client_ip(req: &Request) -> Option<std::net::IpAddr> {
    // Check X-Forwarded-For header
    if let Some(forwarded) = req.headers().get("x-forwarded-for") {
        if let Ok(forwarded_str) = forwarded.to_str() {
            if let Some(first_ip) = forwarded_str.split(',').next() {
                if let Ok(ip) = first_ip.trim().parse() {
                    return Some(ip);
                }
            }
        }
    }

    // Check X-Real-IP header
    if let Some(real_ip) = req.headers().get("x-real-ip") {
        if let Ok(real_ip_str) = real_ip.to_str() {
            if let Ok(ip) = real_ip_str.parse() {
                return Some(ip);
            }
        }
    }

    None
}

/// Enhanced rate limiting middleware with multi-tier support
pub async fn multi_tier_rate_limit_middleware(
    State(state): State<Arc<GatewayState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Check if rate limiting is enabled
    let rate_limit_enabled = {
        let config = state.config.read().await;
        config.security.rate_limit.enabled
    };

    if !rate_limit_enabled {
        return Ok(next.run(req).await);
    }

    // Get user identifier
    let user_id = {
        // Try Bearer token first
        let auth_header = req.headers().get("authorization");
        if let Some(header_value) = auth_header {
            if let Ok(header_str) = header_value.to_str() {
                if let Some(token) = header_str.strip_prefix("Bearer ") {
                    if let Some(session) = state.auth_manager.validate_session(token).await {
                        session.user_id
                    } else {
                        // Try session cookie
                        let cookie_config = SessionCookieConfig::default();
                        if let Some(token) = extract_session_cookie(&req, &cookie_config.name) {
                            if let Some(session) = state.auth_manager.validate_session(&token).await
                            {
                                session.user_id
                            } else {
                                extract_client_ip(&req)
                                    .map(|ip| UserId::new(format!("ip:{}", ip)))
                                    .unwrap_or_else(|| UserId::new("anonymous"))
                            }
                        } else {
                            extract_client_ip(&req)
                                .map(|ip| UserId::new(format!("ip:{}", ip)))
                                .unwrap_or_else(|| UserId::new("anonymous"))
                        }
                    }
                } else {
                    extract_client_ip(&req)
                        .map(|ip| UserId::new(format!("ip:{}", ip)))
                        .unwrap_or_else(|| UserId::new("anonymous"))
                }
            } else {
                extract_client_ip(&req)
                    .map(|ip| UserId::new(format!("ip:{}", ip)))
                    .unwrap_or_else(|| UserId::new("anonymous"))
            }
        } else {
            // Try session cookie for OAuth users
            let cookie_config = SessionCookieConfig::default();
            if let Some(token) = extract_session_cookie(&req, &cookie_config.name) {
                if let Some(session) = state.auth_manager.validate_session(&token).await {
                    session.user_id
                } else {
                    extract_client_ip(&req)
                        .map(|ip| UserId::new(format!("ip:{}", ip)))
                        .unwrap_or_else(|| UserId::new("anonymous"))
                }
            } else {
                extract_client_ip(&req)
                    .map(|ip| UserId::new(format!("ip:{}", ip)))
                    .unwrap_or_else(|| UserId::new("anonymous"))
            }
        }
    };

    let ip = extract_client_ip(&req);
    let endpoint = req.uri().path().to_string();

    // Check multi-tier rate limit using the shared instance from GatewayState
    let result = state.multi_tier_rate_limiter.check(&user_id, ip, &endpoint).await;

    match result {
        MultiTierResult::Allowed { remaining } => {
            let mut response = next.run(req).await;
            let headers = response.headers_mut();
            headers.insert("X-RateLimit-Limit", "100".parse().expect("failed to parse header value"));
            headers.insert("X-RateLimit-Remaining", remaining.to_string().parse().expect("failed to parse header value"));
            Ok(response)
        }
        MultiTierResult::Denied { tier, retry_after_secs } => {
            warn!("Rate limit exceeded for user: {} on tier: {}", user_id, tier);
            let mut response = Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .body(Body::from(format!(
                    "Rate limit exceeded on tier '{}'. Retry after {} seconds.",
                    tier, retry_after_secs
                )))
                .expect("failed to build response");
            response
                .headers_mut()
                .insert("Retry-After", retry_after_secs.to_string().parse().expect("failed to parse header value"));
            response
                .headers_mut()
                .insert("X-RateLimit-Tier", tier.parse().expect("failed to parse header value"));
            Ok(response)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_tier_allowed() {
        let limiter = MultiTierRateLimiter::default();
        let user = UserId::new("user1");
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { limiter.check(&user, None, "/api/test").await });
        assert!(result.is_allowed());
    }

    #[test]
    fn test_multi_tier_per_user_limit() {
        let config = MultiTierRateLimitConfig {
            per_user: TierConfig {
                enabled: true,
                capacity: 2,
                window_secs: 60,
            },
            ..Default::default()
        };
        let limiter = MultiTierRateLimiter::new(config);
        let user = UserId::new("user1");

        let rt = tokio::runtime::Runtime::new().unwrap();
        // First 2 requests allowed
        for _ in 0..2 {
            let result = rt.block_on(limiter.check(&user, None, "/api/test"));
            assert!(result.is_allowed());
        }
        // 3rd request denied
        let result = rt.block_on(limiter.check(&user, None, "/api/test"));
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_multi_tier_stats() {
        let limiter = MultiTierRateLimiter::default();
        let stats = limiter.stats();
        assert_eq!(stats.global_windows, 0);
    }

    #[test]
    fn test_multi_tier_result_is_allowed() {
        let allowed = MultiTierResult::Allowed { remaining: 5 };
        assert!(allowed.is_allowed());
        assert_eq!(allowed.retry_after(), None);

        let denied = MultiTierResult::Denied {
            tier: "global",
            retry_after_secs: 30,
        };
        assert!(!denied.is_allowed());
        assert_eq!(denied.retry_after(), Some(30));
    }

    #[test]
    fn test_config_default() {
        let config = MultiTierRateLimitConfig::default();
        assert!(config.global.enabled);
        assert_eq!(config.global.capacity, 1000);
        assert!(config.per_user.enabled);
        assert!(config.per_ip.enabled);
        assert!(!config.per_endpoint.enabled);
    }

    #[test]
    fn test_multi_tier_ip_limit() {
        let config = MultiTierRateLimitConfig {
            per_ip: TierConfig {
                enabled: true,
                capacity: 2,
                window_secs: 60,
            },
            ..Default::default()
        };
        let limiter = MultiTierRateLimiter::new(config);
        let user = UserId::new("ip:192.168.1.1");

        let rt = tokio::runtime::Runtime::new().unwrap();
        for _ in 0..2 {
            let result =
                rt.block_on(limiter.check(&user, Some("192.168.1.1".parse().unwrap()), "/api"));
            assert!(result.is_allowed());
        }
        let result =
            rt.block_on(limiter.check(&user, Some("192.168.1.1".parse().unwrap()), "/api"));
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_multi_tier_cleanup() {
        let limiter = MultiTierRateLimiter::default();
        let user = UserId::new("user1");

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let _ = limiter.check(&user, None, "/api").await;
        });

        assert!(limiter.stats().global_windows > 0);
        limiter.cleanup();
        // After cleanup, windows may be empty depending on timing
    }
}
