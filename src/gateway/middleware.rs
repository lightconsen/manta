//! Gateway Middleware - Access Control
//!
//! Provides middleware for restricting access to admin APIs:
//! - Localhost-only (127.0.0.1, ::1)
//! - Tailscale network detection
//! - API key authentication (optional)
//! - Rate limiting

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use std::net::IpAddr;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::gateway::GatewayState;
use crate::security::UserId;

/// Allowed network origins for admin APIs
#[derive(Debug, Clone, Default)]
pub enum AllowedOrigin {
    /// Only localhost
    #[default]
    Localhost,
    /// Tailscale network (100.64.0.0/10 CGNAT range)
    Tailscale,
    /// Any private network (RFC 1918)
    Private,
    /// Specific IP addresses
    IpList(Vec<IpAddr>),
    /// Any origin (disable restriction)
    Any,
}

/// Check if an IP address is allowed based on origin policy
#[allow(dead_code)]
fn is_ip_allowed(addr: IpAddr, allowed: &AllowedOrigin) -> bool {
    match allowed {
        AllowedOrigin::Any => true,
        AllowedOrigin::Localhost => is_localhost(addr),
        AllowedOrigin::Tailscale => is_tailscale(addr),
        AllowedOrigin::Private => is_private_ip(addr),
        AllowedOrigin::IpList(allowed_ips) => allowed_ips.contains(&addr),
    }
}

/// Check if IP is localhost
fn is_localhost(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

/// Check if IP is in Tailscale's CGNAT range (100.64.0.0/10)
fn is_tailscale(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            // 100.64.0.0/10 = 100.64.0.0 - 100.127.255.255
            octets[0] == 100 && (octets[1] & 0xC0) == 0x40
        }
        IpAddr::V6(_) => false, // Tailscale uses IPv4 CGNAT
    }
}

/// Check if IP is in a private network (RFC 1918)
fn is_private_ip(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback(),
    }
}

/// Extract client IP from request
fn extract_client_ip(req: &Request) -> Option<IpAddr> {
    // Check X-Forwarded-For header (if behind proxy)
    if let Some(forwarded) = req.headers().get("x-forwarded-for") {
        if let Ok(forwarded_str) = forwarded.to_str() {
            // Take the first IP in the chain
            if let Some(first_ip) = forwarded_str.split(',').next() {
                if let Ok(ip) = first_ip.trim().parse() {
                    debug!("Client IP from X-Forwarded-For: {}", ip);
                    return Some(ip);
                }
            }
        }
    }

    // Check X-Real-IP header
    if let Some(real_ip) = req.headers().get("x-real-ip") {
        if let Ok(real_ip_str) = real_ip.to_str() {
            if let Ok(ip) = real_ip_str.parse() {
                debug!("Client IP from X-Real-IP: {}", ip);
                return Some(ip);
            }
        }
    }

    // Get from connection info (if available in extensions)
    // This requires the ConnectInfo extractor in Axum
    None
}

/// Middleware: Restrict to localhost only
pub async fn localhost_only_middleware(req: Request, next: Next) -> Result<Response, StatusCode> {
    // Extract client IP
    let client_ip = extract_client_ip(&req);

    match client_ip {
        Some(ip) if is_localhost(ip) => {
            debug!("Localhost access granted for: {:?}", req.uri());
            Ok(next.run(req).await)
        }
        Some(ip) => {
            warn!("Non-localhost access attempt to admin API from: {} - {:?}", ip, req.uri());
            Err(StatusCode::FORBIDDEN)
        }
        None => {
            // If we can't determine the IP, check if it's from a Unix socket
            // or allow based on connection type
            debug!("Cannot determine client IP, allowing (may be Unix socket)");
            Ok(next.run(req).await)
        }
    }
}

/// Middleware: Restrict to Tailscale network
pub async fn tailscale_only_middleware(req: Request, next: Next) -> Result<Response, StatusCode> {
    let client_ip = extract_client_ip(&req);

    match client_ip {
        Some(ip) if is_tailscale(ip) || is_localhost(ip) => {
            debug!("Tailscale/localhost access granted for: {:?}", req.uri());
            Ok(next.run(req).await)
        }
        Some(ip) => {
            warn!("Non-Tailscale access attempt to admin API from: {} - {:?}", ip, req.uri());
            Err(StatusCode::FORBIDDEN)
        }
        None => {
            debug!("Cannot determine client IP, allowing");
            Ok(next.run(req).await)
        }
    }
}

/// Middleware: Restrict to private networks
pub async fn private_only_middleware(req: Request, next: Next) -> Result<Response, StatusCode> {
    let client_ip = extract_client_ip(&req);

    match client_ip {
        Some(ip) if is_private_ip(ip) => {
            debug!("Private network access granted for: {:?}", req.uri());
            Ok(next.run(req).await)
        }
        Some(ip) => {
            warn!("Public network access attempt to admin API from: {} - {:?}", ip, req.uri());
            Err(StatusCode::FORBIDDEN)
        }
        None => {
            debug!("Cannot determine client IP, allowing");
            Ok(next.run(req).await)
        }
    }
}

/// Middleware: Authentication check
///
/// Validates Bearer token from Authorization header.
/// If security.auth_required is false, allows all requests.
pub async fn auth_middleware(
    State(state): State<Arc<GatewayState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Check if auth is required
    let auth_required = {
        let config = state.config.read().await;
        config.security.auth_required
    };

    if !auth_required {
        debug!("Auth not required, allowing request");
        return Ok(next.run(req).await);
    }

    // Extract Authorization header
    let auth_header = req.headers().get("authorization");

    match auth_header {
        Some(header_value) => {
            if let Ok(header_str) = header_value.to_str() {
                if let Some(token) = header_str.strip_prefix("Bearer ") {
                    // Validate session
                    if state.auth_manager.validate_session(token).await.is_some() {
                        debug!("Valid auth token, allowing request");
                        return Ok(next.run(req).await);
                    }
                }
            }
            warn!("Invalid or expired auth token");
            Err(StatusCode::UNAUTHORIZED)
        }
        None => {
            warn!("Missing Authorization header");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Middleware: Rate limiting
///
/// Supports both legacy token bucket and multi-tier sliding window rate limiting.
/// Multi-tier mode checks global, per-user, per-IP, and per-endpoint limits.
/// Adds X-RateLimit-* headers to responses.
pub async fn rate_limit_middleware(
    State(state): State<Arc<GatewayState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Check if rate limiting is enabled
    let (rate_limit_enabled, use_multi_tier) = {
        let config = state.config.read().await;
        (config.security.rate_limit.enabled, config.security.rate_limit.multi_tier)
    };

    if !rate_limit_enabled {
        return Ok(next.run(req).await);
    }

    // Get user identifier (from auth token if available, else IP)
    let user_id = {
        let auth_header = req.headers().get("authorization");
        if let Some(header_value) = auth_header {
            if let Ok(header_str) = header_value.to_str() {
                if let Some(token) = header_str.strip_prefix("Bearer ") {
                    if let Some(session) = state.auth_manager.validate_session(token).await {
                        session.user_id
                    } else {
                        extract_client_ip(&req)
                            .map(|ip| UserId::new(ip.to_string()))
                            .unwrap_or_else(|| UserId::new("anonymous"))
                    }
                } else {
                    extract_client_ip(&req)
                        .map(|ip| UserId::new(ip.to_string()))
                        .unwrap_or_else(|| UserId::new("anonymous"))
                }
            } else {
                extract_client_ip(&req)
                    .map(|ip| UserId::new(ip.to_string()))
                    .unwrap_or_else(|| UserId::new("anonymous"))
            }
        } else {
            extract_client_ip(&req)
                .map(|ip| UserId::new(ip.to_string()))
                .unwrap_or_else(|| UserId::new("anonymous"))
        }
    };

    if use_multi_tier {
        // Multi-tier sliding window rate limiting
        let ip = extract_client_ip(&req);
        let endpoint = req.uri().path().to_string();

        let result = state
            .multi_tier_rate_limiter
            .check(&user_id, ip, &endpoint)
            .await;

        match result {
            crate::gateway::rate_limit::MultiTierResult::Allowed { remaining } => {
                let mut response = next.run(req).await;
                let headers = response.headers_mut();
                headers.insert("X-RateLimit-Limit", "100".parse().unwrap());
                headers.insert("X-RateLimit-Remaining", remaining.to_string().parse().unwrap());
                Ok(response)
            }
            crate::gateway::rate_limit::MultiTierResult::Denied { tier, retry_after_secs } => {
                warn!("Rate limit exceeded for user: {} on tier: {}", user_id, tier);
                let mut response = Response::builder()
                    .status(StatusCode::TOO_MANY_REQUESTS)
                    .body(Body::from(format!(
                        "Rate limit exceeded on tier '{}'. Retry after {} seconds.",
                        tier, retry_after_secs
                    )))
                    .unwrap();
                response
                    .headers_mut()
                    .insert("Retry-After", retry_after_secs.to_string().parse().unwrap());
                response
                    .headers_mut()
                    .insert("X-RateLimit-Tier", tier.parse().unwrap());
                Ok(response)
            }
        }
    } else {
        // Legacy token bucket rate limiting
        let result = state.rate_limiter.check(&user_id).await;

        match result {
            crate::security::RateLimitResult::Allowed { remaining, reset_after_secs } => {
                let mut response = next.run(req).await;

                let headers = response.headers_mut();
                headers.insert(
                    "X-RateLimit-Limit",
                    state
                        .rate_limiter
                        .get_state(&user_id)
                        .await
                        .map(|s| s.capacity)
                        .unwrap_or(100)
                        .to_string()
                        .parse()
                        .unwrap(),
                );
                headers.insert("X-RateLimit-Remaining", remaining.to_string().parse().unwrap());
                headers.insert("X-RateLimit-Reset", reset_after_secs.to_string().parse().unwrap());

                Ok(response)
            }
            crate::security::RateLimitResult::Denied { retry_after_secs } => {
                warn!("Rate limit exceeded for user: {}", user_id);
                let mut response = Response::builder()
                    .status(StatusCode::TOO_MANY_REQUESTS)
                    .body(Body::from(format!(
                        "Rate limit exceeded. Retry after {} seconds.",
                        retry_after_secs
                    )))
                    .unwrap();

                response
                    .headers_mut()
                    .insert("Retry-After", retry_after_secs.to_string().parse().unwrap());

                Ok(response)
            }
        }
    }
}

/// Generate a random CSP nonce (16 bytes hex = 32 chars)
fn generate_nonce() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// CSP policy configuration per route type
#[derive(Debug, Clone)]
pub enum CspPolicy {
    /// Strict default policy
    Strict,
    /// API-specific policy (no inline restrictions)
    Api,
    /// Web terminal / admin UI policy with nonce support
    Admin { nonce: String },
}

impl CspPolicy {
    /// Build the CSP string
    pub fn to_header_value(&self) -> String {
        match self {
            CspPolicy::Strict => "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' ws: wss:; frame-ancestors 'none'; base-uri 'self'; form-action 'self';".to_string(),
            CspPolicy::Api => "default-src 'none'; frame-ancestors 'none';".to_string(),
            CspPolicy::Admin { nonce } => format!(
                "default-src 'self'; script-src 'self' 'nonce-{nonce}'; style-src 'self' 'nonce-{nonce}' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self'; connect-src 'self' ws: wss:; frame-ancestors 'none'; base-uri 'self'; form-action 'self';",
                nonce = nonce
            ),
        }
    }
}

/// Middleware: Security headers
///
/// Adds comprehensive security headers to all responses, including:
/// - Content-Security-Policy (with nonce for admin routes)
/// - X-Content-Type-Options
/// - X-Frame-Options
/// - Referrer-Policy
/// - Permissions-Policy
/// - Strict-Transport-Security
pub async fn security_headers_middleware(req: Request, next: Next) -> Response {
    let path = req.uri().path();

    // Determine CSP policy based on route
    let csp_policy = if path.starts_with("/api/") {
        CspPolicy::Api
    } else if path.starts_with("/admin") || path.starts_with("/terminal") {
        CspPolicy::Admin { nonce: generate_nonce() }
    } else {
        CspPolicy::Strict
    };

    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    // Content Security Policy with route-aware strategy
    headers.insert("Content-Security-Policy", csp_policy.to_header_value().parse().unwrap());

    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert("Referrer-Policy", "strict-origin-when-cross-origin".parse().unwrap());
    headers.insert(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=()".parse().unwrap(),
    );
    headers.insert(
        "Strict-Transport-Security",
        "max-age=31536000; includeSubDomains".parse().unwrap(),
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_is_localhost() {
        assert!(is_localhost(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_localhost(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 53))));
        assert!(!is_localhost(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn test_is_localhost_ipv6() {
        assert!(is_localhost(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))));
        assert!(!is_localhost(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 2))));
    }

    #[test]
    fn test_is_tailscale() {
        // 100.64.0.0/10 range
        assert!(is_tailscale(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_tailscale(IpAddr::V4(Ipv4Addr::new(100, 100, 50, 25))));
        assert!(is_tailscale(IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255))));

        // Outside range
        assert!(!is_tailscale(IpAddr::V4(Ipv4Addr::new(100, 63, 255, 255))));
        assert!(!is_tailscale(IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1))));
        assert!(!is_tailscale(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn test_is_tailscale_ipv6() {
        // Tailscale only supports IPv4 CGNAT
        assert!(!is_tailscale(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))));
    }

    #[test]
    fn test_is_private_ip() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    #[test]
    fn test_is_private_ip_ipv6() {
        assert!(is_private_ip(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))));
        assert!(!is_private_ip(IpAddr::V6(Ipv6Addr::new(0x2001, 0x4860, 0, 0, 0, 0, 0, 0x8888))));
    }

    #[test]
    fn test_allowed_origin_default() {
        assert!(matches!(AllowedOrigin::default(), AllowedOrigin::Localhost));
    }

    #[test]
    fn test_is_ip_allowed_any() {
        assert!(is_ip_allowed(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), &AllowedOrigin::Any));
    }

    #[test]
    fn test_is_ip_allowed_localhost() {
        assert!(is_ip_allowed(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            &AllowedOrigin::Localhost
        ));
        assert!(!is_ip_allowed(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            &AllowedOrigin::Localhost
        ));
    }

    #[test]
    fn test_is_ip_allowed_tailscale() {
        assert!(is_ip_allowed(
            IpAddr::V4(Ipv4Addr::new(100, 64, 1, 1)),
            &AllowedOrigin::Tailscale
        ));
        assert!(!is_ip_allowed(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            &AllowedOrigin::Tailscale
        ));
    }

    #[test]
    fn test_is_ip_allowed_private() {
        assert!(is_ip_allowed(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), &AllowedOrigin::Private));
        assert!(!is_ip_allowed(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), &AllowedOrigin::Private));
    }

    #[test]
    fn test_is_ip_allowed_ip_list() {
        let allowed = AllowedOrigin::IpList(vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        ]);
        assert!(is_ip_allowed(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), &allowed));
        assert!(!is_ip_allowed(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), &allowed));
    }

    #[test]
    fn test_extract_client_ip_x_forwarded_for() {
        let mut req = Request::new(Body::empty());
        req.headers_mut()
            .insert("x-forwarded-for", "203.0.113.195, 70.41.3.18".parse().unwrap());
        let ip = extract_client_ip(&req);
        assert_eq!(ip, Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 195))));
    }

    #[test]
    fn test_extract_client_ip_x_real_ip() {
        let mut req = Request::new(Body::empty());
        req.headers_mut()
            .insert("x-real-ip", "192.168.1.1".parse().unwrap());
        let ip = extract_client_ip(&req);
        assert_eq!(ip, Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn test_extract_client_ip_x_forwarded_for_priority() {
        let mut req = Request::new(Body::empty());
        req.headers_mut()
            .insert("x-forwarded-for", "10.0.0.1".parse().unwrap());
        req.headers_mut()
            .insert("x-real-ip", "192.168.1.1".parse().unwrap());
        let ip = extract_client_ip(&req);
        // X-Forwarded-For takes priority
        assert_eq!(ip, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    fn test_extract_client_ip_no_headers() {
        let req = Request::new(Body::empty());
        let ip = extract_client_ip(&req);
        assert_eq!(ip, None);
    }

    #[test]
    fn test_extract_client_ip_invalid() {
        let mut req = Request::new(Body::empty());
        req.headers_mut()
            .insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        let ip = extract_client_ip(&req);
        assert_eq!(ip, None);
    }

    #[test]
    fn test_csp_policy_strict() {
        let policy = CspPolicy::Strict;
        let header = policy.to_header_value();
        assert!(header.contains("default-src 'self'"));
        assert!(header.contains("script-src 'self'"));
        assert!(header.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn test_csp_policy_api() {
        let policy = CspPolicy::Api;
        let header = policy.to_header_value();
        assert!(header.contains("default-src 'none'"));
        assert!(header.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn test_csp_policy_admin() {
        let policy = CspPolicy::Admin { nonce: "abc123".to_string() };
        let header = policy.to_header_value();
        assert!(header.contains("script-src 'self' 'nonce-abc123'"));
        assert!(header.contains("style-src 'self' 'nonce-abc123'"));
        assert!(header.contains("img-src 'self' data: blob:"));
    }

    #[test]
    fn test_generate_nonce() {
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        assert_eq!(nonce1.len(), 32); // 16 bytes hex = 32 chars
        assert_eq!(nonce2.len(), 32);
        assert_ne!(nonce1, nonce2); // Very unlikely to collide
                                    // Should be valid hex
        assert!(hex::decode(&nonce1).is_ok());
    }
}
