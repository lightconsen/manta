//! Configuration loading, migration, validation and environment overrides.

use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::error::{ConfigError, Result};

use super::types::{RE_ENV_VAR, RE_HHMM};
use super::{
    AppConfig, Config, LogFormat, ServiceConfig, StorageType, CURRENT_SCHEMA_VERSION,
    DEFAULT_CONFIG_FILE, ENV_PREFIX,
};

impl Config {
    /// Load configuration from default sources
    ///
    /// The configuration is loaded in the following order (later sources
    /// override earlier ones):
    /// 1. Default values
    /// 2. Config file (config.toml or specified path)
    /// 3. Environment variables (SYSCITY_*)
    pub fn load() -> Result<Self> {
        Self::load_with_file(None::<&std::path::Path>)
    }

    /// Load configuration with a specific config file
    pub fn load_with_file<P: AsRef<Path>>(path: Option<P>) -> Result<Self> {
        // Start with defaults
        let mut config = Config::default();

        // Load from file if available
        let config_path = path
            .as_ref()
            .map(|p| p.as_ref().to_path_buf())
            .or_else(Self::find_config_file);

        if let Some(path) = config_path {
            debug!(path = %path.display(), "Loading config from file");
            match Self::load_from_file(&path) {
                Ok(file_config) => {
                    config = file_config;
                    info!(path = %path.display(), "Loaded config from file");
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "Failed to load config file");
                }
            }
        }

        // Migrate config if schema version is outdated
        if config.schema_version < CURRENT_SCHEMA_VERSION {
            config = Self::migrate(config)?;
        }

        // Override with environment variables
        config.load_from_env()?;

        // Validate the configuration
        config.validate()?;

        Ok(config)
    }

    /// Load configuration from a file
    fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|e| ConfigError::FileRead {
            path: path.to_path_buf(),
            source: e,
        })?;

        // Interpolate environment variables ($VAR / ${VAR}) in the raw TOML
        let contents = Self::interpolate_env_vars(&contents);

        let config: Config = toml::from_str(&contents)
            .map_err(|e| ConfigError::Parse(format!("Invalid TOML: {}", e)))?;

        // Re-attach app config since it was skipped during deserialization
        let mut config = config;
        config.app = AppConfig::default();

        Ok(config)
    }

    /// Find the config file in standard locations
    fn find_config_file() -> Option<PathBuf> {
        let candidates = [
            PathBuf::from(DEFAULT_CONFIG_FILE),
            PathBuf::from(format!(".config/{}", DEFAULT_CONFIG_FILE)),
            // Centralized ~/.syscity/config.toml
            crate::dirs::default_config_file(),
            // Legacy location for backwards compatibility
            dirs::config_dir()
                .map(|d| d.join("syscity").join(DEFAULT_CONFIG_FILE))
                .unwrap_or_default(),
        ];

        for path in &candidates {
            if path.exists() {
                return Some(path.clone());
            }
        }

        None
    }

    /// Load configuration from environment variables
    fn load_from_env(&mut self) -> Result<()> {
        // Server config from env
        if let Ok(host) = std::env::var(format!("{}_SERVER_HOST", ENV_PREFIX)) {
            self.server.host = host;
        }
        if let Ok(port) = std::env::var(format!("{}_SERVER_PORT", ENV_PREFIX)) {
            self.server.port = port.parse().map_err(|e| ConfigError::InvalidValue {
                key: "server.port".to_string(),
                message: format!("Invalid port number: {}", e),
            })?;
        }

        // Logging config from env
        if let Ok(level) = std::env::var(format!("{}_LOG_LEVEL", ENV_PREFIX)) {
            self.logging.level = level;
        }
        if let Ok(format) = std::env::var(format!("{}_LOG_FORMAT", ENV_PREFIX)) {
            self.logging.format = match format.to_lowercase().as_str() {
                "json" => LogFormat::Json,
                "pretty" => LogFormat::Pretty,
                "compact" => LogFormat::Compact,
                _ => {
                    return Err(ConfigError::InvalidValue {
                        key: "logging.format".to_string(),
                        message: format!("Unknown log format: {}", format),
                    }
                    .into())
                }
            };
        }

        // Storage config from env
        if let Ok(storage_type) = std::env::var(format!("{}_STORAGE_TYPE", ENV_PREFIX)) {
            self.storage.storage_type = match storage_type.to_lowercase().as_str() {
                "memory" => StorageType::Memory,
                "file" => StorageType::File,
                "sqlite" => StorageType::Sqlite,
                "database" | "db" => StorageType::Database,
                _ => {
                    return Err(ConfigError::InvalidValue {
                        key: "storage.storage_type".to_string(),
                        message: format!("Unknown storage type: {}", storage_type),
                    }
                    .into())
                }
            };
        }
        if let Ok(conn) = std::env::var(format!("{}_STORAGE_CONNECTION", ENV_PREFIX)) {
            self.storage.connection = Some(conn);
        }

        // Browser config from env
        #[cfg(feature = "browser")]
        {
            if let Ok(val) = std::env::var(format!("{}_BROWSER_BRIDGE_ENABLED", ENV_PREFIX)) {
                self.browser.bridge_enabled =
                    val.parse().map_err(|e| ConfigError::InvalidValue {
                        key: "browser.bridge_enabled".to_string(),
                        message: format!("Invalid boolean: {}", e),
                    })?;
            }
            if let Ok(port) = std::env::var(format!("{}_BROWSER_BRIDGE_PORT", ENV_PREFIX)) {
                self.browser.bridge_port = port.parse().map_err(|e| ConfigError::InvalidValue {
                    key: "browser.bridge_port".to_string(),
                    message: format!("Invalid port number: {}", e),
                })?;
            }
        }

        // Memory config from env
        if let Ok(val) = std::env::var(format!("{}_MEMORY_MULTIMODAL_ENABLED", ENV_PREFIX)) {
            self.memory.multimodal.enabled =
                val.parse().map_err(|e| ConfigError::InvalidValue {
                    key: "memory.multimodal.enabled".to_string(),
                    message: format!("Invalid boolean: {}", e),
                })?;
        }
        if let Ok(val) = std::env::var(format!("{}_MEMORY_MULTIMODAL_MAX_BYTES", ENV_PREFIX)) {
            self.memory.multimodal.max_file_bytes =
                val.parse().map_err(|e| ConfigError::InvalidValue {
                    key: "memory.multimodal.max_file_bytes".to_string(),
                    message: format!("Invalid number: {}", e),
                })?;
        }
        if let Ok(val) = std::env::var(format!("{}_MEMORY_DREAMING_ENABLED", ENV_PREFIX)) {
            self.memory.dreaming.enabled = val.parse().map_err(|e| ConfigError::InvalidValue {
                key: "memory.dreaming.enabled".to_string(),
                message: format!("Invalid boolean: {}", e),
            })?;
        }
        if let Ok(val) = std::env::var(format!("{}_MEMORY_DREAMING_FREQUENCY", ENV_PREFIX)) {
            self.memory.dreaming.frequency = val;
        }
        if let Ok(val) = std::env::var(format!("{}_MEMORY_TIER_ENABLED", ENV_PREFIX)) {
            self.memory.tier.enabled = val.parse().map_err(|e| ConfigError::InvalidValue {
                key: "memory.tier.enabled".to_string(),
                message: format!("Invalid boolean: {}", e),
            })?;
        }
        if let Ok(val) = std::env::var(format!("{}_MEMORY_EFFECTIVENESS_ENABLED", ENV_PREFIX)) {
            self.memory.effectiveness.enabled =
                val.parse().map_err(|e| ConfigError::InvalidValue {
                    key: "memory.effectiveness.enabled".to_string(),
                    message: format!("Invalid boolean: {}", e),
                })?;
        }

        // Computer config from env
        if let Ok(val) = std::env::var(format!("{}_COMPUTER_ENABLED", ENV_PREFIX)) {
            self.computer.enabled = val.parse().map_err(|e| ConfigError::InvalidValue {
                key: "computer.enabled".to_string(),
                message: format!("Invalid boolean: {}", e),
            })?;
        }
        if let Ok(val) = std::env::var(format!("{}_REMOTE_CONTROL_HOST", ENV_PREFIX)) {
            self.computer.remote_control.host = Some(val);
        }
        if let Ok(val) = std::env::var(format!("{}_REMOTE_CONTROL_USER", ENV_PREFIX)) {
            self.computer.remote_control.user = Some(val);
        }
        if let Ok(val) = std::env::var(format!("{}_REMOTE_CONTROL_PORT", ENV_PREFIX)) {
            self.computer.remote_control.port =
                val.parse().map_err(|e| ConfigError::InvalidValue {
                    key: "computer.remote_control.port".to_string(),
                    message: format!("Invalid port number: {}", e),
                })?;
        }
        if let Ok(val) = std::env::var(format!("{}_REMOTE_CONTROL_PROTOCOL", ENV_PREFIX)) {
            self.computer.remote_control.protocol = val;
        }
        if let Ok(val) = std::env::var(format!("{}_REMOTE_CONTROL_KEY_PATH", ENV_PREFIX)) {
            self.computer.remote_control.key_path = Some(val);
        }
        if let Ok(val) = std::env::var(format!("{}_REMOTE_CONTROL_DISPLAY", ENV_PREFIX)) {
            self.computer.remote_control.display = Some(val);
        }
        if let Ok(val) = std::env::var(format!("{}_HEADLESS_ENABLED", ENV_PREFIX)) {
            self.computer.headless.enabled =
                val.parse().map_err(|e| ConfigError::InvalidValue {
                    key: "computer.headless.enabled".to_string(),
                    message: format!("Invalid boolean: {}", e),
                })?;
        }
        if let Ok(val) = std::env::var(format!("{}_HEADLESS_DISPLAY", ENV_PREFIX)) {
            self.computer.headless.display = val;
        }

        Ok(())
    }

    /// Interpolate `$VAR` and `${VAR}` patterns in a raw config string.
    ///
    /// Unknown variables are left as-is (no error) so that fields using `$`
    /// for other purposes are not silently broken.  Variables that reference
    /// other env vars are **not** recursively resolved.
    fn interpolate_env_vars(input: &str) -> String {
        // Match both ${VAR} and $VAR — but not $$ (escaped dollar)
        RE_ENV_VAR
            .replace_all(input, |caps: &regex::Captures<'_>| {
                // $$VAR → literal $VAR (escape)
                if let Some(escaped) = caps.name("escaped") {
                    return format!("${}", escaped.as_str());
                }

                let var_name = caps
                    .name("braced")
                    .or_else(|| caps.name("plain"))
                    .map(|m| m.as_str())
                    .unwrap_or_default();

                match std::env::var(var_name) {
                    Ok(val) => val,
                    Err(_) => {
                        warn!(var = %var_name, "Config env var not set, leaving as-is");
                        caps["full"].to_string()
                    }
                }
            })
            .into_owned()
    }

    /// Migrate configuration from an older schema version to the current one.
    ///
    /// This applies sequential migrations (v0→v1, v1→v2, etc.) so that
    /// configs loaded from older files are brought up to date before use.
    fn migrate(mut config: Self) -> Result<Self> {
        let from_version = config.schema_version;
        let target = CURRENT_SCHEMA_VERSION;

        // v0 → v1: no breaking changes, just establishes the framework
        // (future migrations would be added here as new schema versions are introduced)
        if from_version == 0 {
            // No structural changes needed for v0→v1
        }

        config.schema_version = target;
        info!("Migrated configuration from schema v{} to v{}", from_version, target);

        Ok(config)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        // Validate server config
        if self.server.port == 0 {
            return Err(ConfigError::InvalidValue {
                key: "server.port".to_string(),
                message: "Port cannot be 0".to_string(),
            }
            .into());
        }

        // Validate logging config
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.logging.level.to_lowercase().as_str()) {
            return Err(ConfigError::InvalidValue {
                key: "logging.level".to_string(),
                message: format!("Invalid log level: {}", self.logging.level),
            }
            .into());
        }

        // Validate browser config
        #[cfg(feature = "browser")]
        if self.browser.bridge_port == 0 {
            return Err(ConfigError::InvalidValue {
                key: "browser.bridge_port".to_string(),
                message: "Bridge port cannot be 0".to_string(),
            }
            .into());
        }

        // ── Cross-field validation ────────────────────────────────────

        // Storage: external database type requires a connection string
        if matches!(self.storage.storage_type, StorageType::Database)
            && self.storage.connection.is_none()
        {
            return Err(ConfigError::InvalidValue {
                key: "storage.connection".to_string(),
                message: "Connection string is required when storage type is 'database'"
                    .to_string(),
            }
            .into());
        }

        // Heartbeat: active hours must be a valid HH:MM format
        if self.heartbeat.enabled {
            if !RE_HHMM.is_match(&self.heartbeat.active_hours_start) {
                return Err(ConfigError::InvalidValue {
                    key: "heartbeat.active_hours_start".to_string(),
                    message: format!(
                        "Invalid time format '{}', expected HH:MM",
                        self.heartbeat.active_hours_start
                    ),
                }
                .into());
            }
            if !RE_HHMM.is_match(&self.heartbeat.active_hours_end) {
                return Err(ConfigError::InvalidValue {
                    key: "heartbeat.active_hours_end".to_string(),
                    message: format!(
                        "Invalid time format '{}', expected HH:MM",
                        self.heartbeat.active_hours_end
                    ),
                }
                .into());
            }
        }

        // Validate computer config
        let valid_protocols = ["ssh", "vnc", "rdp"];
        if !valid_protocols.contains(&self.computer.remote_control.protocol.as_str()) {
            return Err(ConfigError::InvalidValue {
                key: "computer.remote_control.protocol".to_string(),
                message: format!(
                    "Invalid protocol: {}. Must be one of: ssh, vnc, rdp",
                    self.computer.remote_control.protocol
                ),
            }
            .into());
        }
        if self.computer.remote_control.port == 0 {
            return Err(ConfigError::InvalidValue {
                key: "computer.remote_control.port".to_string(),
                message: "Remote control port cannot be 0".to_string(),
            }
            .into());
        }

        Ok(())
    }

    /// Get the server address (host:port)
    pub fn server_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }

    /// Get a service configuration by name
    pub fn get_service(&self, name: &str) -> Option<&ServiceConfig> {
        self.services.get(name)
    }

    /// Check if a service is configured
    pub fn has_service(&self, name: &str) -> bool {
        self.services.contains_key(name)
    }

    /// Resolve all secrets in the configuration
    ///
    /// This resolves SecretRef values to their actual secret values using
    /// environment variables, files, or external executables.
    pub async fn resolve_secrets(&mut self) -> Result<()> {
        use crate::secrets::SecretResolver;

        let resolver = SecretResolver::default();

        // Resolve secrets in all service configurations
        for (name, service) in &mut self.services {
            if let Some(api_key_ref) = &service.api_key {
                match resolver.resolve(api_key_ref).await {
                    Ok(resolved) => {
                        debug!("Resolved API key for service '{}'", name);
                        // Store the resolved value back as a raw string SecretRef
                        service.api_key = Some(crate::secrets::SecretRef::String(resolved));
                    }
                    Err(e) => {
                        warn!("Failed to resolve API key for service '{}': {}", name, e);
                        // Continue with other services, don't fail completely
                    }
                }
            }
        }

        Ok(())
    }

    /// Get a resolved API key for a service
    ///
    /// Returns None if the service doesn't exist or has no API key.
    /// Returns the raw value (resolved or inline) if available.
    pub fn get_resolved_api_key(&self, service_name: &str) -> Option<String> {
        self.services.get(service_name).and_then(|s| {
            s.api_key.as_ref().and_then(|key| match key {
                crate::secrets::SecretRef::String(s) => Some(s.clone()),
                _ => None, // Not yet resolved
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrate_v0_to_v1() {
        let mut config = Config::default();
        config.schema_version = 0;
        let migrated = Config::migrate(config).unwrap();
        assert_eq!(migrated.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn test_no_migrate_needed() {
        let config = Config::default();
        assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
        // migrate is a no-op when already at current version
        let migrated = Config::migrate(config).unwrap();
        assert_eq!(migrated.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn test_server_addr() {
        let config = Config::default();
        assert_eq!(config.server_addr(), "127.0.0.1:8080");
    }

    #[test]
    fn test_validate_valid_config() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_port() {
        let mut config = Config::default();
        config.server.port = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_invalid_log_level() {
        let mut config = Config::default();
        config.logging.level = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    #[cfg(feature = "browser")]
    fn test_browser_config_validate() {
        let mut config = Config::default();
        config.browser.bridge_port = 0;
        assert!(config.validate().is_err());

        config.browser.bridge_port = 18800;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_computer_config_invalid_protocol() {
        let mut config = Config::default();
        config.computer.remote_control.protocol = "invalid".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_computer_config_invalid_port() {
        let mut config = Config::default();
        config.computer.remote_control.port = 0;
        assert!(config.validate().is_err());
    }
}
