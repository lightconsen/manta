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
use crate::security::{RateLimitHeaders, UserId};

/// Allowed network origins for admin APIs
#[derive(Debug, Clone)]
pub enum AllowedOrigin {
    /// Only localhost
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

impl Default for AllowedOrigin {
    fn default() -> Self {
        // Default: localhost and Tailscale
        AllowedOrigin::Localhost
    }
}

/// Check if an IP address is allowed based on origin policy
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
                if header_str.starts_with("Bearer ") {
                    let token = &header_str[7..];
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
/// Uses token bucket algorithm per user (identified by IP or user ID from auth).
/// Adds X-RateLimit-* headers to responses.
pub async fn rate_limit_middleware(
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

    // Get user identifier (from auth token if available, else IP)
    let user_id = {
        let auth_header = req.headers().get("authorization");
        if let Some(header_value) = auth_header {
            if let Ok(header_str) = header_value.to_str() {
                if header_str.starts_with("Bearer ") {
                    let token = &header_str[7..];
                    if let Some(session) = state.auth_manager.validate_session(token).await {
                        session.user_id
                    } else {
                        // Invalid token, use IP
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

    // Check rate limit
    let result = state.rate_limiter.check(&user_id).await;

    match result {
        crate::security::RateLimitResult::Allowed { remaining, reset_after_secs } => {
            let mut response = next.run(req).await;

            // Add rate limit headers
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

            // Add retry-after header
            response
                .headers_mut()
                .insert("Retry-After", retry_after_secs.to_string().parse().unwrap());

            Ok(response)
        }
    }
}

/// Middleware: Security headers
///
/// Adds security headers to all responses
pub async fn security_headers_middleware(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;

    let headers = response.headers_mut();
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert("X-Frame-Options", "DENY".parse().unwrap());
    headers.insert("X-XSS-Protection", "1; mode=block".parse().unwrap());
    headers.insert("Referrer-Policy", "strict-origin-when-cross-origin".parse().unwrap());

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_is_localhost() {
        assert!(is_localhost(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_localhost(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 53))));
        assert!(!is_localhost(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
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
    fn test_is_private_ip() {
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_private_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(!is_private_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }
}
