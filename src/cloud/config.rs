use serde::{Deserialize, Serialize};

/// Syscity Cloud integration config (§2.7 / docs/cloud-integration.md).
///
/// Runtime gate: only active when `enabled = true` **and** a session token is
/// present (double gate). The session token itself lives in the SecretStore,
/// not in config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Master runtime switch for the cloud path.
    #[serde(default)]
    pub enabled: bool,
    /// Cloud API base (OpenAI-compatible `/v1/*` + `/api/v1/*`).
    #[serde(default = "default_api_base")]
    pub api_base: String,
    /// Engine web URL the cloud OAuth callback lands on after login.
    #[serde(default = "default_redirect_base")]
    pub redirect_base: String,
}

fn default_api_base() -> String {
    "https://api.syscity.net".to_string()
}

fn default_redirect_base() -> String {
    "http://localhost:18080/cloud/login/callback".to_string()
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_base: default_api_base(),
            redirect_base: default_redirect_base(),
        }
    }
}
