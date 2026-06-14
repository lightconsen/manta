//! Trusted proxy authentication for Syscity Gateway.
//!
//! When the gateway sits behind a reverse proxy (e.g. nginx, Traefik, Tailscale
//! funnel), the proxy terminates TLS and forwards the original user identity in
//! HTTP headers. This module validates that the direct connection comes from a
//! trusted proxy, requires configured headers, extracts the user identity, and
//! enforces an allowlist.
//!
//! # Configuration
//!
//! ```toml
//! [security.trusted_proxy]
//! enabled = true
//! trusted_proxies = ["127.0.0.1", "10.0.0.0/8", "192.168.1.10"]
//! required_headers = ["X-Forwarded-User", "Tailscale-User-Login"]
//! allow_users = ["alice@example.com", "bob", "admin_*"]
//! ```

use axum::http::{HeaderMap, Request};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;
use tracing::{debug, warn};

/// Configuration for trusted proxy authentication.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TrustedProxyConfig {
    /// Enable trusted proxy authentication.
    #[serde(default)]
    pub enabled: bool,
    /// List of trusted proxy IPs or CIDR networks (e.g. "127.0.0.1", "10.0.0.0/8").
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// Headers to inspect for user identity, in priority order.
    /// The first header present and non-empty wins.
    #[serde(default)]
    pub required_headers: Vec<String>,
    /// Allowlist of user identities. Empty means all extracted users are allowed.
    /// Supports simple `*` wildcard at the start or end of a pattern.
    #[serde(default)]
    pub allow_users: Vec<String>,
}

impl TrustedProxyConfig {
    /// Create a disabled config.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Builder-style enable.
    pub fn enabled(mut self) -> Self {
        self.enabled = true;
        self
    }
}

/// Extracted trusted-proxy identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProxyUser {
    /// User identity from the trusted proxy header.
    pub user_id: String,
    /// Header name that provided the identity.
    pub header_name: String,
    /// Direct proxy IP that forwarded the request.
    pub proxy_ip: IpAddr,
}

/// Authentication failure reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustedProxyError {
    /// Direct connection is not from a configured trusted proxy.
    UntrustedProxy { proxy_ip: IpAddr },
    /// A required header is missing.
    MissingHeader { header: String },
    /// No identity header produced a non-empty value.
    NoUserExtracted,
    /// Extracted user is not in the allowlist.
    UserNotAllowed { user_id: String },
}

impl fmt::Display for TrustedProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrustedProxyError::UntrustedProxy { proxy_ip } => {
                write!(f, "connection from untrusted proxy {}", proxy_ip)
            }
            TrustedProxyError::MissingHeader { header } => {
                write!(f, "required trusted-proxy header '{}' is missing", header)
            }
            TrustedProxyError::NoUserExtracted => {
                write!(f, "no trusted-proxy user header produced a value")
            }
            TrustedProxyError::UserNotAllowed { user_id } => {
                write!(f, "user '{}' is not in the trusted-proxy allowlist", user_id)
            }
        }
    }
}

impl std::error::Error for TrustedProxyError {}

/// Trusted proxy authenticator.
#[derive(Debug, Clone)]
pub struct TrustedProxyAuthenticator {
    config: TrustedProxyConfig,
    networks: Vec<IpNet>,
}

impl TrustedProxyAuthenticator {
    /// Create a new authenticator from config.
    pub fn new(config: TrustedProxyConfig) -> Self {
        let networks = parse_networks(&config.trusted_proxies);
        Self { config, networks }
    }

    /// Check if an IP address belongs to a trusted proxy.
    pub fn is_trusted_proxy(&self, ip: &IpAddr) -> bool {
        self.networks.iter().any(|net| net.contains(ip))
    }

    /// Extract the user identity from request headers.
    ///
    /// Iterates `required_headers` in order and returns the first non-empty
    /// value. Returns `None` if none of the configured headers are present.
    pub fn extract_user(&self, headers: &HeaderMap) -> Option<(String, String)> {
        for header_name in &self.config.required_headers {
            if let Some(value) = headers.get(header_name) {
                if let Ok(s) = value.to_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        return Some((header_name.clone(), trimmed.to_string()));
                    }
                }
            }
        }
        None
    }

    /// Check if a user identity matches the allowlist.
    ///
    /// An empty allowlist allows everyone. Patterns support a single `*`
    /// wildcard at the start or end (e.g. `admin_*`, `*@example.com`).
    pub fn is_user_allowed(&self, user_id: &str) -> bool {
        if self.config.allow_users.is_empty() {
            return true;
        }
        let normalized = user_id.to_lowercase();
        self.config
            .allow_users
            .iter()
            .any(|pattern| wildcard_match(pattern, &normalized))
    }

    /// Authenticate a request using the direct proxy IP.
    ///
    /// Returns the extracted user on success, or a descriptive error on failure.
    pub fn authenticate<B>(
        &self,
        req: &Request<B>,
        direct_ip: Option<IpAddr>,
    ) -> Result<TrustedProxyUser, TrustedProxyError> {
        if !self.config.enabled {
            // Should not be invoked when disabled, but fail closed.
            return Err(TrustedProxyError::NoUserExtracted);
        }

        let proxy_ip = direct_ip.ok_or(TrustedProxyError::NoUserExtracted)?;

        if !self.is_trusted_proxy(&proxy_ip) {
            warn!(
                "Trusted proxy auth rejected: direct connection from untrusted IP {}",
                proxy_ip
            );
            return Err(TrustedProxyError::UntrustedProxy { proxy_ip });
        }

        // Require all configured headers to be present (defense in depth).
        for header_name in &self.config.required_headers {
            if !req.headers().contains_key(header_name) {
                warn!(
                    "Trusted proxy auth rejected: missing required header '{}' from {}",
                    header_name, proxy_ip
                );
                return Err(TrustedProxyError::MissingHeader {
                    header: header_name.clone(),
                });
            }
        }

        let Some((header_name, user_id)) = self.extract_user(req.headers()) else {
            warn!(
                "Trusted proxy auth rejected: no user identity extracted from headers from {}",
                proxy_ip
            );
            return Err(TrustedProxyError::NoUserExtracted);
        };

        if !self.is_user_allowed(&user_id) {
            warn!(
                "Trusted proxy auth rejected: user '{}' not in allowlist",
                user_id
            );
            return Err(TrustedProxyError::UserNotAllowed {
                user_id: user_id.clone(),
            });
        }

        debug!(
            "Trusted proxy auth accepted: user '{}' via header '{}' from {}",
            user_id, header_name, proxy_ip
        );

        Ok(TrustedProxyUser {
            user_id,
            header_name,
            proxy_ip,
        })
    }

    /// Get the underlying config.
    pub fn config(&self) -> &TrustedProxyConfig {
        &self.config
    }
}

/// Parse a list of IP addresses or CIDR networks.
fn parse_networks(inputs: &[String]) -> Vec<IpNet> {
    inputs
        .iter()
        .filter_map(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return None;
            }
            // Try CIDR first, then single IP.
            if let Ok(net) = IpNet::from_str(trimmed) {
                return Some(net);
            }
            if let Ok(ip) = IpAddr::from_str(trimmed) {
                let net = IpNet::from(ip);
                return Some(net);
            }
            warn!("Ignoring invalid trusted_proxy network: '{}'", trimmed);
            None
        })
        .collect()
}

/// Simple wildcard matching with a single `*` at the start or end.
fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim().to_lowercase();
    let value = value.to_lowercase();

    if pattern == "*" || pattern.is_empty() {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix('*') {
        return value.starts_with(prefix);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return value.ends_with(suffix);
    }

    pattern == value
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn sample_config() -> TrustedProxyConfig {
        TrustedProxyConfig {
            enabled: true,
            trusted_proxies: vec!["127.0.0.1".to_string(), "10.0.0.0/8".to_string()],
            required_headers: vec!["X-Forwarded-User".to_string()],
            allow_users: vec!["alice".to_string(), "admin_*".to_string()],
        }
    }

    #[test]
    fn test_parse_networks_mixed() {
        let nets = parse_networks(&[
            "127.0.0.1".to_string(),
            "10.0.0.0/8".to_string(),
            "::1".to_string(),
            "bad".to_string(),
            "".to_string(),
        ]);
        assert_eq!(nets.len(), 3);
        assert!(nets.iter().any(|n: &IpNet| n.contains(&"127.0.0.1".parse::<IpAddr>().unwrap())));
        assert!(nets.iter().any(|n: &IpNet| n.contains(&"10.5.5.5".parse::<IpAddr>().unwrap())));
        assert!(nets.iter().any(|n: &IpNet| n.contains(&"::1".parse::<IpAddr>().unwrap())));
    }

    #[test]
    fn test_is_trusted_proxy() {
        let auth = TrustedProxyAuthenticator::new(sample_config());
        assert!(auth.is_trusted_proxy(&"127.0.0.1".parse().unwrap()));
        assert!(auth.is_trusted_proxy(&"10.255.255.255".parse().unwrap()));
        assert!(!auth.is_trusted_proxy(&"192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn test_extract_user() {
        let auth = TrustedProxyAuthenticator::new(sample_config());
        let mut headers = HeaderMap::new();
        assert!(auth.extract_user(&headers).is_none());

        headers.insert("X-Forwarded-User", HeaderValue::from_static("alice"));
        let (name, user) = auth.extract_user(&headers).unwrap();
        assert_eq!(name, "X-Forwarded-User");
        assert_eq!(user, "alice");
    }

    #[test]
    fn test_extract_user_priority() {
        let config = TrustedProxyConfig {
            enabled: true,
            trusted_proxies: vec!["127.0.0.1".to_string()],
            required_headers: vec![
                "Tailscale-User-Login".to_string(),
                "X-Forwarded-User".to_string(),
            ],
            allow_users: Vec::new(),
        };
        let auth = TrustedProxyAuthenticator::new(config);
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-User", HeaderValue::from_static("bob"));
        headers.insert("Tailscale-User-Login", HeaderValue::from_static("alice"));

        let (name, user) = auth.extract_user(&headers).unwrap();
        assert_eq!(name, "Tailscale-User-Login");
        assert_eq!(user, "alice");
    }

    #[test]
    fn test_is_user_allowed() {
        let auth = TrustedProxyAuthenticator::new(sample_config());
        assert!(auth.is_user_allowed("alice"));
        assert!(auth.is_user_allowed("ALICE")); // case-insensitive
        assert!(auth.is_user_allowed("admin_jim"));
        assert!(!auth.is_user_allowed("bob"));
        assert!(!auth.is_user_allowed("jim_admin"));
    }

    #[test]
    fn test_empty_allowlist_allows_all() {
        let config = TrustedProxyConfig {
            enabled: true,
            trusted_proxies: vec!["127.0.0.1".to_string()],
            required_headers: vec!["X-Forwarded-User".to_string()],
            allow_users: Vec::new(),
        };
        let auth = TrustedProxyAuthenticator::new(config);
        assert!(auth.is_user_allowed("anyone"));
    }

    #[test]
    fn test_authenticate_success() {
        let auth = TrustedProxyAuthenticator::new(sample_config());
        let mut req = Request::builder()
            .uri("/api/v1/health")
            .body(())
            .unwrap();
        req.headers_mut()
            .insert("X-Forwarded-User", HeaderValue::from_static("alice"));

        let user = auth.authenticate(&req, Some("127.0.0.1".parse().unwrap())).unwrap();
        assert_eq!(user.user_id, "alice");
        assert_eq!(user.header_name, "X-Forwarded-User");
    }

    #[test]
    fn test_authenticate_untrusted_proxy() {
        let auth = TrustedProxyAuthenticator::new(sample_config());
        let req = Request::builder().uri("/").body(()).unwrap();
        let err = auth
            .authenticate(&req, Some("192.168.1.1".parse().unwrap()))
            .unwrap_err();
        assert!(matches!(err, TrustedProxyError::UntrustedProxy { .. }));
    }

    #[test]
    fn test_authenticate_missing_header() {
        let auth = TrustedProxyAuthenticator::new(sample_config());
        let req = Request::builder().uri("/").body(()).unwrap();
        let err = auth
            .authenticate(&req, Some("127.0.0.1".parse().unwrap()))
            .unwrap_err();
        assert!(matches!(err, TrustedProxyError::MissingHeader { .. }));
    }

    #[test]
    fn test_authenticate_user_not_allowed() {
        let auth = TrustedProxyAuthenticator::new(sample_config());
        let mut req = Request::builder().uri("/").body(()).unwrap();
        req.headers_mut()
            .insert("X-Forwarded-User", HeaderValue::from_static("mallory"));
        let err = auth
            .authenticate(&req, Some("127.0.0.1".parse().unwrap()))
            .unwrap_err();
        assert!(matches!(err, TrustedProxyError::UserNotAllowed { .. }));
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = TrustedProxyConfig {
            enabled: true,
            trusted_proxies: vec!["127.0.0.1".to_string(), "10.0.0.0/8".to_string()],
            required_headers: vec!["X-Forwarded-User".to_string()],
            allow_users: vec!["alice".to_string()],
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: TrustedProxyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn test_default_config_disabled() {
        let config: TrustedProxyConfig = Default::default();
        assert!(!config.enabled);
        assert!(config.trusted_proxies.is_empty());
        assert!(config.required_headers.is_empty());
        assert!(config.allow_users.is_empty());
    }
}
