//! Tailscale Integration for Remote Access
//!
//! Provides built-in Tailscale Serve/Funnel support for secure remote access
//! to the Syscity Gateway without complex network configuration.

use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::process::Command;
use tracing::{error, info, warn};

/// Tailscale configuration
#[derive(Debug, Clone)]
pub struct TailscaleConfig {
    /// Port to expose
    pub port: u16,
    /// Domain (if using funnel)
    pub domain: Option<String>,
    /// Whether to use funnel (public) or just serve (tailnet only)
    pub use_funnel: bool,
}

/// Start Tailscale serve/funnel
pub async fn start(port: u16, domain: Option<String>) -> crate::Result<()> {
    info!("Starting Tailscale integration...");

    // Check if tailscale is installed
    match Command::new("tailscale").arg("version").output().await {
        Ok(_) => info!("Tailscale CLI found"),
        Err(e) => {
            warn!("Tailscale CLI not found: {}", e);
            warn!("Install Tailscale: https://tailscale.com/download");
            return Err(crate::error::SyscityError::ExternalService {
                source: "Tailscale not installed".to_string(),
                cause: Some(Box::new(e)),
            });
        }
    }

    // Determine if we should use funnel (public) or serve (tailnet)
    let use_funnel = domain.is_some();

    if use_funnel {
        // Start funnel for public access
        let domain_str = domain.unwrap_or_default();
        info!("Starting Tailscale funnel on port {} with domain {}", port, domain_str);

        let output = Command::new("tailscale")
            .args([
                "funnel",
                "--http",
                &format!("{}:{}", domain_str, port),
                &port.to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to start Tailscale funnel".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Tailscale funnel failed: {}", stderr);
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Tailscale funnel error: {}", stderr),
                cause: None,
            });
        }

        info!("Tailscale funnel started successfully");
    } else {
        // Start serve for tailnet-only access
        info!("Starting Tailscale serve on port {}", port);

        let output = Command::new("tailscale")
            .args([
                "serve",
                "--http",
                &format!("http://localhost:{}", port),
                &port.to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to start Tailscale serve".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Tailscale serve failed: {}", stderr);
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Tailscale serve error: {}", stderr),
                cause: None,
            });
        }

        info!("Tailscale serve started successfully");
    }

    Ok(())
}

/// Stop Tailscale serve/funnel
pub async fn stop() -> crate::Result<()> {
    info!("Stopping Tailscale serve/funnel...");

    let output = Command::new("tailscale")
        .args(["serve", "off"])
        .output()
        .await
        .map_err(|e| crate::error::SyscityError::ExternalService {
            source: "Failed to stop Tailscale".to_string(),
            cause: Some(Box::new(e)),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Tailscale stop warning: {}", stderr);
    }

    let output = Command::new("tailscale")
        .args(["funnel", "off"])
        .output()
        .await
        .map_err(|e| crate::error::SyscityError::ExternalService {
            source: "Failed to stop Tailscale funnel".to_string(),
            cause: Some(Box::new(e)),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Tailscale funnel stop warning: {}", stderr);
    }

    info!("Tailscale stopped");
    Ok(())
}

/// Structured Tailscale status — parsed from `tailscale status --json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailscaleStatus {
    /// Tailscale version string (e.g. "1.72.0").
    pub version: Option<String>,
    /// Current tailnet name.
    pub current_tailnet: Option<String>,
    /// The current machine's peer info.
    pub self_peer: Option<PeerInfo>,
    /// All peers in the tailnet.
    #[serde(default)]
    pub peers: Vec<PeerInfo>,
    /// Whether the current machine is online.
    #[serde(default)]
    pub online: bool,
}

/// Info about a single Tailscale peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Stable node identifier.
    pub id: String,
    /// Short hostname.
    pub hostname: String,
    /// Fully-qualified DNS name (e.g. "host.tailnet.ts.net").
    pub dns_name: Option<String>,
    /// Operating system (e.g. "linux", "darwin").
    pub os: Option<String>,
    /// Whether this peer is currently online.
    pub online: bool,
    /// Tailscale IP addresses (e.g. ["100.x.x.x"]).
    #[serde(default)]
    pub ip_addresses: Vec<String>,
}

/// Get Tailscale status as a structured `TailscaleStatus`.
///
/// Runs `tailscale status --json` and deserializes the output.
/// Returns an error if the binary is missing, the command fails,
/// or the JSON is malformed (unknown fields are silently ignored
/// for forward compatibility).
pub async fn status() -> crate::Result<TailscaleStatus> {
    let output = Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
        .map_err(|e| crate::error::SyscityError::ExternalService {
            source: "Failed to get Tailscale status".to_string(),
            cause: Some(Box::new(e)),
        })?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: TailscaleStatus = serde_json::from_str(&stdout)
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: format!("Failed to parse Tailscale status JSON: {}", e),
                cause: None,
            })?;
        Ok(parsed)
    } else {
        Err(crate::error::SyscityError::ExternalService {
            source: "Tailscale status failed".to_string(),
            cause: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tailscale_status() {
        let json = r#"{
            "Version": "1.72.0",
            "CurrentTailnet": {"Name": "example.ts.net"},
            "Self": {"ID": "self1", "HostName": "my-machine", "DNSName": "my-machine.example.ts.net", "OS": "darwin", "Online": true, "TailscaleIPs": ["100.1.2.3"]},
            "Peer": [
                {"ID": "peer1", "HostName": "server", "DNSName": "server.example.ts.net", "OS": "linux", "Online": true, "TailscaleIPs": ["100.2.3.4"]},
                {"ID": "peer2", "HostName": "laptop", "DNSName": "laptop.example.ts.net", "OS": "darwin", "Online": false, "TailscaleIPs": ["100.3.4.5"]}
            ]
        }"#;

        let status: TailscaleStatus = serde_json::from_str(json).expect("should parse");
        assert_eq!(status.version, Some("1.72.0".to_string()));
        assert_eq!(status.current_tailnet, Some("example.ts.net".to_string()));
        assert!(status.online);

        let self_peer = status.self_peer.expect("should have self peer");
        assert_eq!(self_peer.hostname, "my-machine");

        assert_eq!(status.peers.len(), 2);
        assert_eq!(status.peers[0].hostname, "server");
        assert!(status.peers[0].online);
        assert!(!status.peers[1].online);
    }

    #[test]
    fn test_parse_tailscale_status_minimal() {
        // Minimal valid response — no peers, no self.
        let json = r#"{
            "Version": "1.72.0",
            "CurrentTailnet": {"Name": "example.ts.net"},
            "Peer": {}
        }"#;

        let status: TailscaleStatus = serde_json::from_str(json).expect("should parse minimal");
        assert_eq!(status.version, Some("1.72.0".to_string()));
        assert!(status.peers.is_empty());
        assert!(status.self_peer.is_none());
    }

    #[test]
    fn test_tailscale_status_extra_fields_ignored() {
        // Forward-compat: unknown fields must not cause parse failure.
        let json = r#"{
            "Version": "1.72.0",
            "CurrentTailnet": {"Name": "example.ts.net"},
            "Peer": {},
            "MagicDNS": true,
            "ExtraField": "ignored"
        }"#;

        let status: TailscaleStatus = serde_json::from_str(json).expect("should ignore extra fields");
        assert_eq!(status.version, Some("1.72.0".to_string()));
    }
}
