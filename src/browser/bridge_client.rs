//! Browser bridge client — HTTP client for connecting to bridge server
//!
//! Used by BrowserTool when BROWSER_BRIDGE_URL env is set.
//! Requires `browser` feature.

use serde::{Deserialize, Serialize};
use tracing::debug;

/// HTTP client for browser bridge
#[derive(Debug, Clone)]
pub struct BridgeClient {
    client: reqwest::Client,
    base_url: String,
    token: String,
}

/// Response from bridge health endpoint
#[derive(Debug, Deserialize)]
pub struct HealthResponse {
    pub status: String,
}

/// Health check result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthStatus {
    /// Server is reachable and healthy
    Healthy,
    /// Server responded but reported unhealthy
    Unhealthy,
    /// Server is not reachable
    Unreachable,
}

/// Response from bridge status endpoint
#[derive(Debug, Deserialize)]
pub struct StatusResponse {
    pub profiles: Vec<ProfileStatus>,
}

/// Per-profile status from bridge
#[derive(Debug, Deserialize)]
pub struct ProfileStatus {
    pub name: String,
    pub page_count: usize,
}

/// Navigate request body
#[derive(Debug, Serialize)]
pub struct NavigateRequest {
    pub profile: String,
    pub url: String,
}

/// Navigate response
#[derive(Debug, Deserialize)]
pub struct NavigateResponse {
    pub success: bool,
    pub target_id: String,
    pub url: String,
    pub title: String,
}

/// Snapshot request body
#[derive(Debug, Serialize)]
pub struct SnapshotRequest {
    pub profile: String,
    pub target_id: String,
    pub max_chars: Option<usize>,
}

/// Snapshot response
#[derive(Debug, Deserialize)]
pub struct SnapshotResponse {
    pub success: bool,
    pub snapshot: String,
    pub url: String,
    pub title: String,
    pub interactive_count: usize,
    pub truncated: bool,
}

/// Act request body
#[derive(Debug, Serialize)]
pub struct ActRequest {
    pub profile: String,
    pub target_id: String,
    pub ref_id: usize,
    #[serde(flatten)]
    pub action: crate::browser::ActKind,
}

/// Act response
#[derive(Debug, Deserialize)]
pub struct ActResponse {
    pub success: bool,
    pub message: String,
}

/// Screenshot request body
#[derive(Debug, Serialize)]
pub struct ScreenshotRequest {
    pub profile: String,
    pub target_id: String,
    pub full_page: Option<bool>,
}

/// Screenshot response
#[derive(Debug, Deserialize)]
pub struct ScreenshotResponse {
    pub success: bool,
    pub format: String,
    pub data: String,
}

/// Generic message response
#[derive(Debug, Deserialize)]
pub struct MessageResponse {
    pub success: bool,
    pub message: String,
}

impl BridgeClient {
    /// Create a new bridge client
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }

    /// Create a client from environment variables
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("BROWSER_BRIDGE_URL").ok()?;
        let token = std::env::var("BROWSER_BRIDGE_TOKEN").unwrap_or_default();
        Some(Self::new(base_url, token))
    }

    /// Check if the bridge is reachable
    pub async fn health_check(&self) -> HealthStatus {
        let url = format!("{}/health", self.base_url);
        match self.client.get(&url).send().await {
            Ok(res) if res.status().is_success() => HealthStatus::Healthy,
            Ok(_res) => {
                debug!("Bridge health returned non-success status");
                HealthStatus::Unhealthy
            }
            Err(e) => {
                debug!("Bridge health check failed: {}", e);
                HealthStatus::Unreachable
            }
        }
    }

    /// Get bridge status (profiles and page counts)
    pub async fn status(&self) -> crate::Result<StatusResponse> {
        let url = format!("{}/status", self.base_url);
        let res = self
            .client
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Bridge status request failed".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !res.status().is_success() {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Bridge status returned {}", res.status()),
                cause: None,
            });
        }

        res.json()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to parse bridge status".to_string(),
                cause: Some(Box::new(e)),
            })
    }

    /// Navigate to a URL
    pub async fn navigate(&self, profile: &str, url: &str) -> crate::Result<NavigateResponse> {
        let req_url = format!("{}/navigate", self.base_url);
        let body = NavigateRequest {
            profile: profile.to_string(),
            url: url.to_string(),
        };

        let res = self
            .client
            .post(&req_url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Bridge navigate request failed".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !res.status().is_success() {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Bridge navigate returned {}", res.status()),
                cause: None,
            });
        }

        res.json()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to parse bridge navigate response".to_string(),
                cause: Some(Box::new(e)),
            })
    }

    /// Take an ARIA snapshot
    pub async fn snapshot(
        &self,
        profile: &str,
        target_id: &str,
        max_chars: Option<usize>,
    ) -> crate::Result<SnapshotResponse> {
        let url = format!("{}/snapshot", self.base_url);
        let body = SnapshotRequest {
            profile: profile.to_string(),
            target_id: target_id.to_string(),
            max_chars,
        };

        let res = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Bridge snapshot request failed".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !res.status().is_success() {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Bridge snapshot returned {}", res.status()),
                cause: None,
            });
        }

        res.json()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to parse bridge snapshot response".to_string(),
                cause: Some(Box::new(e)),
            })
    }

    /// Act on an element by ref_id
    pub async fn act(
        &self,
        profile: &str,
        target_id: &str,
        ref_id: usize,
        action: crate::browser::ActKind,
    ) -> crate::Result<ActResponse> {
        let url = format!("{}/act", self.base_url);
        let body = ActRequest {
            profile: profile.to_string(),
            target_id: target_id.to_string(),
            ref_id,
            action,
        };

        let res = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Bridge act request failed".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !res.status().is_success() {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Bridge act returned {}", res.status()),
                cause: None,
            });
        }

        res.json()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to parse bridge act response".to_string(),
                cause: Some(Box::new(e)),
            })
    }

    /// Take a screenshot
    pub async fn screenshot(
        &self,
        profile: &str,
        target_id: &str,
        full_page: Option<bool>,
    ) -> crate::Result<ScreenshotResponse> {
        let url = format!("{}/screenshot", self.base_url);
        let body = ScreenshotRequest {
            profile: profile.to_string(),
            target_id: target_id.to_string(),
            full_page,
        };

        let res = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Bridge screenshot request failed".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !res.status().is_success() {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Bridge screenshot returned {}", res.status()),
                cause: None,
            });
        }

        res.json()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to parse bridge screenshot response".to_string(),
                cause: Some(Box::new(e)),
            })
    }

    /// Start a browser instance
    pub async fn start(&self, profile: &str) -> crate::Result<MessageResponse> {
        let url = format!("{}/start", self.base_url);
        let body = serde_json::json!({"profile": profile});

        let res = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Bridge start request failed".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !res.status().is_success() {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Bridge start returned {}", res.status()),
                cause: None,
            });
        }

        res.json()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to parse bridge start response".to_string(),
                cause: Some(Box::new(e)),
            })
    }

    /// Stop a browser instance
    pub async fn stop(&self, profile: &str) -> crate::Result<MessageResponse> {
        let url = format!("{}/stop", self.base_url);
        let body = serde_json::json!({"profile": profile});

        let res = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Bridge stop request failed".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !res.status().is_success() {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Bridge stop returned {}", res.status()),
                cause: None,
            });
        }

        res.json()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to parse bridge stop response".to_string(),
                cause: Some(Box::new(e)),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bridge_client_create() {
        let client = BridgeClient::new("http://localhost:18800", "test-token");
        // health_check will return Unreachable because no server is running
        assert_eq!(client.health_check().await, HealthStatus::Unreachable);
    }

    #[test]
    fn test_bridge_client_from_env_missing() {
        // Save any existing env vars so the test is independent of the
        // outside environment (e.g. CI may set BROWSER_BRIDGE_URL).
        let saved_url = std::env::var("BROWSER_BRIDGE_URL").ok();
        let saved_token = std::env::var("BROWSER_BRIDGE_TOKEN").ok();

        std::env::remove_var("BROWSER_BRIDGE_URL");
        std::env::remove_var("BROWSER_BRIDGE_TOKEN");
        assert!(BridgeClient::from_env().is_none());

        // Restore previous values so later tests are not affected.
        match saved_url {
            Some(url) => std::env::set_var("BROWSER_BRIDGE_URL", url),
            None => std::env::remove_var("BROWSER_BRIDGE_URL"),
        }
        match saved_token {
            Some(token) => std::env::set_var("BROWSER_BRIDGE_TOKEN", token),
            None => std::env::remove_var("BROWSER_BRIDGE_TOKEN"),
        }
    }

    #[test]
    fn test_bridge_client_from_env_present() {
        std::env::set_var("BROWSER_BRIDGE_URL", "http://localhost:18800");
        std::env::set_var("BROWSER_BRIDGE_TOKEN", "token123");
        let client = BridgeClient::from_env().unwrap();
        // The client was created successfully
        drop(client);
        std::env::remove_var("BROWSER_BRIDGE_URL");
        std::env::remove_var("BROWSER_BRIDGE_TOKEN");
    }
}
