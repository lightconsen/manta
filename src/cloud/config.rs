use serde::{Deserialize, Serialize};

/// Syscity Cloud integration config (§2.7 / docs/cloud-integration.md).
///
/// Runtime gate: only active when `enabled = true` **and** a session token is
/// present (double gate). The session token itself lives in the SecretStore,
/// not in config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Master runtime switch for the cloud path.
    ///
    /// Defaults to **on** for a binary compiled with the `cloud` feature
    /// (this module only exists then); the actual cloud paths still require a
    /// logged-in session, so an anonymous user just sees the local behavior.
    /// Override with `SYSCITY_CLOUD_ENABLED=0` or config.toml `[cloud]
    /// enabled = false` to force cloud off.
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
            // A cloud-compiled binary is a deliberate choice to ship cloud
            // support — default it on, still overridable via env.
            enabled: match std::env::var("SYSCITY_CLOUD_ENABLED") {
                Ok(v) => v == "1" || v.eq_ignore_ascii_case("true"),
                Err(_) => true,
            },
            api_base: std::env::var("SYSCITY_CLOUD_API_BASE")
                .unwrap_or_else(|_| default_api_base()),
            redirect_base: std::env::var("SYSCITY_CLOUD_REDIRECT_BASE")
                .unwrap_or_else(|_| default_redirect_base()),
        }
    }
}
