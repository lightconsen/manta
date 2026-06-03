//! SSRF Navigation Guard
//!
//! Prevents navigation to private IP addresses and other restricted hosts.
//!
//! Checks:
//! - URL scheme must be http/https/about
//! - IP literals are checked against private ranges
//! - Hostnames are checked against blocklist/allowlist

use std::net::IpAddr;

/// Navigation policy for URL validation
#[derive(Debug, Clone)]
pub struct NavigationPolicy {
    /// Allow navigation to private IPs
    pub allow_private: bool,
    /// Allowed hostnames (empty = all public allowed)
    pub allowed_hostnames: Vec<String>,
    /// Blocked hostnames
    pub blocked_hostnames: Vec<String>,
}

impl Default for NavigationPolicy {
    fn default() -> Self {
        Self {
            allow_private: false,
            allowed_hostnames: Vec::new(),
            blocked_hostnames: vec!["localhost".to_string()],
        }
    }
}

impl NavigationPolicy {
    /// Create a permissive policy (allows private IPs)
    pub fn permissive() -> Self {
        Self {
            allow_private: true,
            ..Default::default()
        }
    }

    /// Create a restrictive policy (default)
    pub fn restrictive() -> Self {
        Self::default()
    }
}

/// Validate that a URL is allowed by the navigation policy
pub fn assert_navigation_allowed(url: &str, policy: &NavigationPolicy) -> crate::Result<()> {
    let parsed = url::Url::parse(url)
        .map_err(|e| crate::error::SyscityError::Validation(format!("Invalid URL: {}", e)))?;

    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https" | "about") {
        return Err(crate::error::SyscityError::Validation(format!(
            "URL scheme '{}' is not allowed",
            scheme
        )));
    }

    let host_raw = parsed.host_str().unwrap_or("");
    // Strip brackets from IPv6 literals (url::Url returns "[::1]" for IPv6)
    let host = if host_raw.starts_with('[') && host_raw.ends_with(']') {
        &host_raw[1..host_raw.len() - 1]
    } else {
        host_raw
    };

    // Check blocked hostnames
    if policy
        .blocked_hostnames
        .iter()
        .any(|h| host.eq_ignore_ascii_case(h))
    {
        return Err(crate::error::SyscityError::Validation(format!(
            "Hostname '{}' is blocked",
            host
        )));
    }

    // Check IP literal
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !policy.allow_private && is_private_ip(ip) {
            return Err(crate::error::SyscityError::Validation(format!(
                "Navigation to private IP '{}' is not allowed",
                ip
            )));
        }
    }

    // Check allowlist (if specified, only allowlisted hosts are permitted)
    if !policy.allowed_hostnames.is_empty()
        && !policy
            .allowed_hostnames
            .iter()
            .any(|h| host.eq_ignore_ascii_case(h))
    {
        return Err(crate::error::SyscityError::Validation(format!(
            "Hostname '{}' is not in the allowed list",
            host
        )));
    }

    Ok(())
}

/// Check if an IP address is in a private range
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                // 0.0.0.0/8
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                // fc00::/7 (unique local addresses)
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navigation_guard_blocks_private_ip() {
        let policy = NavigationPolicy::restrictive();

        assert!(assert_navigation_allowed("http://127.0.0.1/", &policy).is_err());
        assert!(assert_navigation_allowed("http://10.0.0.1/", &policy).is_err());
        assert!(assert_navigation_allowed("http://192.168.1.1/", &policy).is_err());
        assert!(assert_navigation_allowed("http://172.16.0.1/", &policy).is_err());
        assert!(assert_navigation_allowed("http://[::1]/", &policy).is_err());
        assert!(assert_navigation_allowed("http://localhost/", &policy).is_err());
    }

    #[test]
    fn test_navigation_guard_allows_public() {
        let policy = NavigationPolicy::restrictive();

        assert!(assert_navigation_allowed("https://example.com/", &policy).is_ok());
        assert!(assert_navigation_allowed("https://google.com/", &policy).is_ok());
        assert!(assert_navigation_allowed("https://github.com/", &policy).is_ok());
    }

    #[test]
    fn test_navigation_guard_blocks_invalid_scheme() {
        let policy = NavigationPolicy::restrictive();

        assert!(assert_navigation_allowed("file:///etc/passwd", &policy).is_err());
        assert!(assert_navigation_allowed("ftp://example.com/", &policy).is_err());
    }

    #[test]
    fn test_navigation_guard_permissive() {
        let policy = NavigationPolicy::permissive();

        assert!(assert_navigation_allowed("http://127.0.0.1/", &policy).is_ok());
        assert!(assert_navigation_allowed("http://192.168.1.1/", &policy).is_ok());
    }

    #[test]
    fn test_navigation_guard_allowlist() {
        let policy = NavigationPolicy {
            allow_private: false,
            allowed_hostnames: vec!["example.com".to_string()],
            blocked_hostnames: Vec::new(),
        };

        assert!(assert_navigation_allowed("https://example.com/", &policy).is_ok());
        assert!(assert_navigation_allowed("https://google.com/", &policy).is_err());
    }

    #[test]
    fn test_is_private_ip() {
        use std::net::Ipv4Addr;

        assert!(is_private_ip(Ipv4Addr::new(127, 0, 0, 1).into()));
        assert!(is_private_ip(Ipv4Addr::new(10, 0, 0, 1).into()));
        assert!(is_private_ip(Ipv4Addr::new(192, 168, 1, 1).into()));
        assert!(is_private_ip(Ipv4Addr::new(172, 16, 0, 1).into()));
        assert!(!is_private_ip(Ipv4Addr::new(8, 8, 8, 8).into()));
        assert!(!is_private_ip(Ipv4Addr::new(1, 1, 1, 1).into()));
    }
}
