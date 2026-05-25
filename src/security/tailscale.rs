//! Tailscale authentication for Manta Gateway
//!
//! Verifies connections using the Tailscale local API (`tailscale whois`)
//! and caches results to avoid repeated lookups.

use serde::Deserialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// User information returned by Tailscale whois lookup.
#[derive(Debug, Clone, Deserialize)]
pub struct TailscaleUser {
    /// Login name (e.g., "user@example.com")
    #[serde(rename = "loginName")]
    pub login_name: String,
    /// Display name
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// Node name
    #[serde(rename = "nodeName")]
    pub node_name: String,
    /// Tailnet name
    pub tailnet: String,
}

/// Tailscale authenticator with caching.
pub struct TailscaleAuthenticator {
    cache: Arc<RwLock<HashMap<String, (TailscaleUser, Instant)>>>,
    cache_ttl: Duration,
}

impl Default for TailscaleAuthenticator {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

impl TailscaleAuthenticator {
    /// Create a new authenticator with the given cache TTL.
    pub fn new(cache_ttl: Duration) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl,
        }
    }

    /// Authenticate an IP address using Tailscale whois.
    ///
    /// Returns `Some(TailscaleUser)` if the user is verified, or `None`
    /// if verification fails or Tailscale is not available.
    pub async fn authenticate(&self, ip: &str) -> Option<TailscaleUser> {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some((user, timestamp)) = cache.get(ip) {
                if timestamp.elapsed() < self.cache_ttl {
                    debug!("Tailscale auth cache hit for: {}", ip);
                    return Some(user.clone());
                }
            }
        }

        // Try local API first, then fallback to CLI
        let user = whois_local_api(ip).await.or_else(|| {
            debug!("Tailscale local API failed, trying CLI fallback");
            whois_cli(ip)
        })?;

        debug!("Tailscale auth verified: {} ({})", user.login_name, user.node_name);

        // Cache the result
        let mut cache = self.cache.write().await;
        cache.insert(ip.to_string(), (user.clone(), Instant::now()));

        Some(user)
    }

    /// Check if an IP is authorized for the given tailnets.
    ///
    /// Returns `true` if the user's tailnet matches one of the allowed tailnets.
    /// If `allowed_tailnets` is empty, any Tailscale user is allowed.
    pub async fn is_authorized(&self, ip: &str, allowed_tailnets: &[String]) -> bool {
        let Some(user) = self.authenticate(ip).await else {
            return false;
        };

        if allowed_tailnets.is_empty() {
            return true;
        }

        allowed_tailnets.contains(&user.tailnet)
    }

    /// Clear the authentication cache.
    pub async fn clear_cache(&self) {
        self.cache.write().await.clear();
    }
}

/// Query Tailscale local API for user info.
///
/// Uses the Unix socket API at `http://local-tailscaled.sock/localapi/v0/whois`.
async fn whois_local_api(ip: &str) -> Option<TailscaleUser> {
    #[cfg(unix)]
    {
        // Try to connect to the Tailscale daemon socket
        let socket_path = "/var/run/tailscale/tailscaled.sock";
        let stream = std::fs::metadata(socket_path).ok()?;
        if !stream.is_file() {
            return None;
        }

        // Use reqwest with Unix socket connector (if available)
        // Fallback: use curl via shell command
        let url = format!("http://127.0.0.1/localapi/v0/whois?addr={}", ip);
        debug!("Querying Tailscale local API: {}", url);

        // Try using the Unix socket directly via a custom HTTP client
        match tokio::net::UnixStream::connect(socket_path).await {
            Ok(_stream) => {
                // Stream is valid, but reqwest doesn't natively support Unix sockets.
                // Use the CLI fallback instead.
                debug!("Tailscale socket found, using CLI fallback for whois");
                whois_cli(ip)
            }
            Err(e) => {
                debug!("Tailscale socket connect failed: {}", e);
                None
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = ip;
        None
    }
}

/// Query Tailscale whois via CLI command.
fn whois_cli(ip: &str) -> Option<TailscaleUser> {
    use std::process::Command;

    debug!("Running tailscale whois for: {}", ip);

    let output = Command::new("tailscale")
        .args(["whois", "--json", ip])
        .output()
        .ok()?;

    if !output.status.success() {
        warn!(
            "tailscale whois failed for {}: {:?}",
            ip,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    debug!("tailscale whois output: {}", stdout.trim());

    // Parse the JSON response
    // The CLI output format is different from the API
    // It returns a JSON object with a "UserProfile" field
    let parsed: serde_json::Value = serde_json::from_str(&stdout).ok()?;

    let login_name = parsed
        .get("UserProfile")
        .and_then(|p| p.get("LoginName"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let display_name = parsed
        .get("UserProfile")
        .and_then(|p| p.get("DisplayName"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let node_name = parsed
        .get("Node")
        .and_then(|n| n.get("Name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let tailnet = parsed
        .get("UserProfile")
        .and_then(|p| p.get("LoginName"))
        .and_then(|v| v.as_str())
        .map(|s| {
            // Extract tailnet from login name (user@tailnet.ts.net -> tailnet.ts.net)
            if let Some(at_idx) = s.rfind('@') {
                s[at_idx + 1..].to_string()
            } else {
                s.to_string()
            }
        })
        .unwrap_or_default();

    if login_name.is_empty() {
        return None;
    }

    Some(TailscaleUser {
        login_name,
        display_name,
        node_name,
        tailnet,
    })
}

/// Extract client IP from an axum request.
pub fn extract_client_ip(req: &axum::extract::Request) -> Option<IpAddr> {
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

/// Check if an IP is in Tailscale's CGNAT range (100.64.0.0/10).
pub fn is_tailscale_ip(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            octets[0] == 100 && (octets[1] & 0xC0) == 0x40
        }
        IpAddr::V6(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_tailscale_ip_valid() {
        assert!(is_tailscale_ip("100.64.0.1".parse().unwrap()));
        assert!(is_tailscale_ip("100.127.255.255".parse().unwrap()));
        assert!(is_tailscale_ip("100.100.100.100".parse().unwrap()));
    }

    #[test]
    fn test_is_tailscale_ip_invalid() {
        assert!(!is_tailscale_ip("192.168.1.1".parse().unwrap()));
        assert!(!is_tailscale_ip("10.0.0.1".parse().unwrap()));
        assert!(!is_tailscale_ip("100.63.255.255".parse().unwrap()));
        assert!(!is_tailscale_ip("100.128.0.0".parse().unwrap()));
    }

    #[tokio::test]
    async fn test_authenticator_cache() {
        let auth = TailscaleAuthenticator::new(Duration::from_secs(60));
        // Cache miss, tailscale CLI may or may not be available
        let result = auth.authenticate("100.64.0.1").await;
        // Result depends on whether tailscale CLI is installed
        // On CI/dev machines without tailscale, this will be None
        if result.is_some() {
            let user = result.unwrap();
            assert!(!user.login_name.is_empty());
            // Second call should hit cache
            let result2 = auth.authenticate("100.64.0.1").await;
            assert!(result2.is_some());
            assert_eq!(result2.unwrap().login_name, user.login_name);
        }
    }
}
