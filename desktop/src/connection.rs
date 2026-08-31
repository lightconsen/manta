//! Desktop client connection settings: local (embedded/reused gateway) vs
//! remote (a gateway running on another host, authenticated with a token).
//!
//! Stored in `~/.syscity/client.toml`, separate from `config.toml` (which the
//! gateway itself does not reliably parse).

use serde::{Deserialize, Serialize};

/// Which gateway this desktop app should use.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMode {
    /// Run / reuse a local gateway (loopback).
    #[default]
    Local,
    /// Connect to a gateway on another host.
    Remote,
}

/// Client connection settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectionConfig {
    pub mode: ConnectionMode,
    /// Remote gateway host (ignored in Local mode).
    pub host: String,
    /// Remote gateway port (ignored in Local mode).
    pub port: u16,
    /// Remote gateway shared token (ignored in Local mode).
    pub token: Option<String>,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            mode: ConnectionMode::Local,
            host: "127.0.0.1".to_string(),
            port: 18080,
            token: None,
        }
    }
}

/// Path to the connection settings file.
pub fn connection_file() -> std::path::PathBuf {
    syscity::dirs::config_dir().join("client.toml")
}

/// Load connection settings, falling back to Local defaults.
pub fn load_connection() -> ConnectionConfig {
    let path = connection_file();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(config) = toml::from_str::<ConnectionConfig>(&content) {
            return config;
        }
    }
    ConnectionConfig::default()
}

/// Persist connection settings.
pub fn save_connection(config: &ConnectionConfig) -> Result<(), String> {
    let path = connection_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = toml::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

/// The remote gateway base URL (`http://host:port`).
pub fn remote_base(config: &ConnectionConfig) -> String {
    format!("http://{}:{}", config.host, config.port)
}
