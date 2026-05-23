//! Browser sandbox — Docker-isolated browser with noVNC
//!
//! Optional P3 feature. Runs browser in a Docker container for isolation.
//! Requires `browser` feature.

use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

/// Docker sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Docker image to use
    #[serde(default = "default_image")]
    pub image: String,
    /// VNC port
    #[serde(default = "default_vnc_port")]
    pub vnc_port: u16,
    /// CDP port
    #[serde(default = "default_cdp_port")]
    pub cdp_port: u16,
    /// noVNC port
    #[serde(default = "default_novnc_port")]
    pub novnc_port: u16,
    /// Container memory limit in MB
    #[serde(default = "default_memory_limit")]
    pub memory_limit_mb: u32,
    /// Container CPU limit (e.g., "1.0" for 1 core)
    #[serde(default = "default_cpu_limit")]
    pub cpu_limit: String,
}

fn default_image() -> String {
    "browserless/chrome:latest".to_string()
}

fn default_vnc_port() -> u16 {
    5900
}

fn default_cdp_port() -> u16 {
    9222
}

fn default_novnc_port() -> u16 {
    6080
}

fn default_memory_limit() -> u32 {
    2048
}

fn default_cpu_limit() -> String {
    "1.0".to_string()
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            image: default_image(),
            vnc_port: default_vnc_port(),
            cdp_port: default_cdp_port(),
            novnc_port: default_novnc_port(),
            memory_limit_mb: default_memory_limit(),
            cpu_limit: default_cpu_limit(),
        }
    }
}

/// Browser sandbox manager
#[derive(Debug, Clone)]
pub struct BrowserSandbox {
    config: SandboxConfig,
}

impl BrowserSandbox {
    /// Create a new sandbox
    pub fn new(config: SandboxConfig) -> Self {
        Self { config }
    }

    /// Get the sandbox configuration
    pub fn config(&self) -> &SandboxConfig {
        &self.config
    }

    /// Start the sandbox container
    pub async fn start(&self) -> crate::Result<SandboxInfo> {
        info!("Starting browser sandbox container");

        // Check if Docker is available
        match tokio::process::Command::new("docker")
            .args(["--version"])
            .output()
            .await
        {
            Ok(output) if output.status.success() => {
                debug!("Docker is available");
            }
            _ => {
                return Err(crate::error::MantaError::ExternalService {
                    source: "Docker is not available".to_string(),
                    cause: None,
                });
            }
        }

        let container_name = format!("manta-browser-sandbox-{}", uuid::Uuid::new_v4());
        let cdp_port = self.config.cdp_port;
        let vnc_port = self.config.vnc_port;
        let novnc_port = self.config.novnc_port;
        let memory = format!("{}m", self.config.memory_limit_mb);

        let args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--rm".to_string(),
            "--name".to_string(),
            container_name.clone(),
            "-p".to_string(),
            format!("{}:9222", cdp_port),
            "-p".to_string(),
            format!("{}:5900", vnc_port),
            "-p".to_string(),
            format!("{}:6080", novnc_port),
            "--memory".to_string(),
            memory,
            "--cpus".to_string(),
            self.config.cpu_limit.clone(),
            "--cap-drop".to_string(),
            "ALL".to_string(),
            "--cap-add".to_string(),
            "SYS_ADMIN".to_string(),
            "--security-opt".to_string(),
            "seccomp=unconfined".to_string(),
            self.config.image.clone(),
        ];

        debug!("Docker run args: {:?}", args);

        let output = tokio::process::Command::new("docker")
            .args(&args)
            .output()
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to start Docker container".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Docker container start failed: {}", stderr);
            return Err(crate::error::MantaError::ExternalService {
                source: format!("Docker container start failed: {}", stderr),
                cause: None,
            });
        }

        let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        info!(container_id = %container_id, "Browser sandbox container started");

        // Wait a moment for Chrome to start inside the container
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        Ok(SandboxInfo {
            container_id,
            container_name,
            cdp_url: format!("http://127.0.0.1:{}", cdp_port),
            vnc_port,
            novnc_port,
        })
    }

    /// Stop the sandbox container
    pub async fn stop(&self, container_id: &str) -> crate::Result<()> {
        info!(container_id = %container_id, "Stopping browser sandbox container");

        let output = tokio::process::Command::new("docker")
            .args(["stop", "-t", "10", container_id])
            .output()
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to stop Docker container".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!("Docker stop warning: {}", stderr);
        } else {
            info!(container_id = %container_id, "Browser sandbox container stopped");
        }

        Ok(())
    }

    /// Check if a container is running
    pub async fn is_running(&self, container_id: &str) -> crate::Result<bool> {
        let output = tokio::process::Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", container_id])
            .output()
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to inspect Docker container".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !output.status.success() {
            return Ok(false);
        }

        let running = String::from_utf8_lossy(&output.stdout).trim() == "true";
        Ok(running)
    }
}

/// Information about a running sandbox container
#[derive(Debug, Clone)]
pub struct SandboxInfo {
    /// Docker container ID
    pub container_id: String,
    /// Docker container name
    pub container_name: String,
    /// CDP URL for connecting to Chrome inside the container
    pub cdp_url: String,
    /// Exposed VNC port
    pub vnc_port: u16,
    /// Exposed noVNC port
    pub novnc_port: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_config_default() {
        let config = SandboxConfig::default();
        assert_eq!(config.image, "browserless/chrome:latest");
        assert_eq!(config.vnc_port, 5900);
        assert_eq!(config.cdp_port, 9222);
        assert_eq!(config.novnc_port, 6080);
        assert_eq!(config.memory_limit_mb, 2048);
        assert_eq!(config.cpu_limit, "1.0");
    }

    #[test]
    fn test_sandbox_create() {
        let config = SandboxConfig::default();
        let sandbox = BrowserSandbox::new(config.clone());
        assert_eq!(sandbox.config().image, config.image);
    }
}
