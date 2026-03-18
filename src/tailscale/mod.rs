//! Tailscale Integration for Remote Access
//!
//! Provides built-in Tailscale Serve/Funnel support for secure remote access
//! to the Manta Gateway without complex network configuration.

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
            return Err(crate::error::MantaError::ExternalService {
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
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to start Tailscale funnel".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Tailscale funnel failed: {}", stderr);
            return Err(crate::error::MantaError::ExternalService {
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
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to start Tailscale serve".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Tailscale serve failed: {}", stderr);
            return Err(crate::error::MantaError::ExternalService {
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
        .map_err(|e| crate::error::MantaError::ExternalService {
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
        .map_err(|e| crate::error::MantaError::ExternalService {
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

/// Get Tailscale status
pub async fn status() -> crate::Result<String> {
    let output = Command::new("tailscale")
        .args(["status"])
        .output()
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to get Tailscale status".to_string(),
            cause: Some(Box::new(e)),
        })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(crate::error::MantaError::ExternalService {
            source: "Tailscale status failed".to_string(),
            cause: None,
        })
    }
}
