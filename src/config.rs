//! Configuration management for Syscity
//!
//! This module handles loading and validating configuration from
//! multiple sources: defaults, config files, and environment variables.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::{ConfigError, Result};
use crate::secrets::SecretRef;

#[allow(clippy::unwrap_used)]
static RE_ENV_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<full>\$\$(?P<escaped>[\w_]+)|\$\{(?P<braced>\w+)\}|\$(?P<plain>\w+))").unwrap()
});

#[allow(clippy::unwrap_used)]
static RE_HHMM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{2}:\d{2}$").unwrap());

/// Default configuration file name
pub const DEFAULT_CONFIG_FILE: &str = "config.toml";

/// Environment variable prefix
pub const ENV_PREFIX: &str = "SYSCITY";

/// Current configuration schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

fn current_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Configuration schema version for migration support
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,

    /// Application metadata
    #[serde(skip)]
    pub app: AppConfig,

    /// Server configuration
    #[serde(default)]
    pub server: ServerConfig,

    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Storage configuration
    #[serde(default)]
    pub storage: StorageConfig,

    /// External service configurations
    #[serde(default)]
    pub services: HashMap<String, ServiceConfig>,

    /// Browser automation configuration
    #[cfg(feature = "browser")]
    #[serde(default)]
    pub browser: BrowserConfig,

    /// Memory subsystem configuration
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Heartbeat scheduler configuration
    #[serde(default)]
    pub heartbeat: crate::heartbeat::HeartbeatConfig,

    /// Computer / desktop automation configuration
    #[serde(default)]
    pub computer: ComputerConfig,

    /// Standing orders configuration (persistent background agent programs)
    #[serde(default)]
    pub standing_orders: crate::standing_orders::config::StandingOrderConfig,

    /// Capability set configuration (profile, scope, enabled sets)
    #[serde(default)]
    pub capabilities: CapabilitiesConfig,

    /// Custom key-value pairs
    #[serde(flatten)]
    pub extra: HashMap<String, toml::Value>,
}

/// Application metadata
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Application name
    pub name: String,
    /// Application version
    pub version: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            name: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Host to bind to
    #[serde(default = "default_host")]
    pub host: String,
    /// Port to listen on
    #[serde(default = "default_port")]
    pub port: u16,
    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    /// Maximum request body size in bytes
    #[serde(default = "default_max_body_size")]
    pub max_body_size: usize,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_timeout() -> u64 {
    30
}

fn default_max_body_size() -> usize {
    10 * 1024 * 1024 // 10 MB
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            timeout_seconds: default_timeout(),
            max_body_size: default_max_body_size(),
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Log format (json, pretty, compact)
    #[serde(default = "default_log_format")]
    pub format: LogFormat,
    /// Optional log file path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Whether to log to stdout
    #[serde(default = "default_true")]
    pub stdout: bool,
    /// Log rotation configuration
    #[serde(default)]
    pub rotation: LogRotationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Json,
    Pretty,
    Compact,
}

/// Log rotation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRotationConfig {
    /// Enable log rotation
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum file size before rotation (bytes)
    #[serde(default = "default_max_size")]
    pub max_size: u64,
    /// Maximum number of archived files to keep
    #[serde(default = "default_max_files")]
    pub max_files: usize,
}

impl Default for LogRotationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size: 10 * 1024 * 1024, // 10 MB
            max_files: 5,
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> LogFormat {
    LogFormat::Compact
}

fn default_true() -> bool {
    true
}

fn default_max_size() -> u64 {
    10 * 1024 * 1024 // 10 MB
}

fn default_max_files() -> usize {
    5
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
            file: None,
            stdout: true,
            rotation: LogRotationConfig::default(),
        }
    }
}

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Storage type (memory, file, database)
    #[serde(default = "default_storage_type")]
    pub storage_type: StorageType,
    /// Connection string or path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// Database name (for database storage)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageType {
    Memory,
    File,
    #[serde(alias = "sqlite")]
    Database,
}

fn default_storage_type() -> StorageType {
    StorageType::Memory
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage_type: default_storage_type(),
            connection: None,
            database: None,
        }
    }
}

/// Memory subsystem configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryConfig {
    /// Multimodal file storage settings
    #[serde(default)]
    pub multimodal: MemoryMultimodalConfig,
    /// Dreaming engine settings
    #[serde(default)]
    pub dreaming: MemoryDreamingConfig,
    /// Tier system settings
    #[serde(default)]
    pub tier: MemoryTierConfig,
    /// Effectiveness tracking settings
    #[serde(default)]
    pub effectiveness: MemoryEffectivenessConfig,
}

/// Multimodal storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMultimodalConfig {
    /// Enable multimodal storage
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Enabled modalities (image, audio)
    #[serde(default = "default_multimodal_modalities")]
    pub modalities: Vec<String>,
    /// Maximum file size in bytes
    #[serde(default = "default_multimodal_max_bytes")]
    pub max_file_bytes: u64,
}

impl Default for MemoryMultimodalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            modalities: default_multimodal_modalities(),
            max_file_bytes: default_multimodal_max_bytes(),
        }
    }
}

fn default_multimodal_modalities() -> Vec<String> {
    vec!["image".to_string(), "audio".to_string()]
}

fn default_multimodal_max_bytes() -> u64 {
    10 * 1024 * 1024 // 10 MB
}

/// Dreaming engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryDreamingConfig {
    /// Enable dreaming
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Cron expression for scheduling (default: daily at 3 AM)
    #[serde(default = "default_dreaming_frequency")]
    pub frequency: String,
    /// Speed: fast, balanced, slow
    #[serde(default = "default_dreaming_speed")]
    pub speed: String,
    /// Thinking depth: low, medium, high
    #[serde(default = "default_dreaming_thinking")]
    pub thinking: String,
    /// Budget: cheap, medium, expensive
    #[serde(default = "default_dreaming_budget")]
    pub budget: String,
    /// Similarity threshold for deduplication
    #[serde(default = "default_dreaming_dedup_threshold")]
    pub dedup_similarity_threshold: f32,
}

impl Default for MemoryDreamingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            frequency: default_dreaming_frequency(),
            speed: default_dreaming_speed(),
            thinking: default_dreaming_thinking(),
            budget: default_dreaming_budget(),
            dedup_similarity_threshold: default_dreaming_dedup_threshold(),
        }
    }
}

fn default_dreaming_frequency() -> String {
    "0 0 3 * * *".to_string()
}

fn default_dreaming_speed() -> String {
    "balanced".to_string()
}

fn default_dreaming_thinking() -> String {
    "medium".to_string()
}

fn default_dreaming_budget() -> String {
    "medium".to_string()
}

fn default_dreaming_dedup_threshold() -> f32 {
    0.95
}

/// Memory tier configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTierConfig {
    /// Enable tier management
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Auto-promote/demote memories
    #[serde(default = "default_true")]
    pub auto_promote: bool,
    /// Maintenance interval in seconds
    #[serde(default = "default_tier_maintenance_interval")]
    pub maintenance_interval_secs: u64,
}

impl Default for MemoryTierConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_promote: true,
            maintenance_interval_secs: default_tier_maintenance_interval(),
        }
    }
}

fn default_tier_maintenance_interval() -> u64 {
    24 * 60 * 60 // Daily
}

/// Memory effectiveness tracking configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEffectivenessConfig {
    /// Enable effectiveness tracking
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Auto-adjust memory importance based on hit rate
    #[serde(default = "default_true")]
    pub auto_adjust: bool,
    /// Hit rate threshold for promotion (0.0-1.0)
    #[serde(default = "default_effectiveness_promotion_threshold")]
    pub promotion_threshold: f32,
    /// Hit rate threshold for demotion (0.0-1.0)
    #[serde(default = "default_effectiveness_demotion_threshold")]
    pub demotion_threshold: f32,
}

impl Default for MemoryEffectivenessConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_adjust: true,
            promotion_threshold: default_effectiveness_promotion_threshold(),
            demotion_threshold: default_effectiveness_demotion_threshold(),
        }
    }
}

fn default_effectiveness_promotion_threshold() -> f32 {
    0.7
}

fn default_effectiveness_demotion_threshold() -> f32 {
    0.2
}

/// External service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Service endpoint URL
    pub endpoint: String,
    /// API key (can be raw string, env var reference like "$ENV_VAR", or
    /// SecretRef object)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretRef>,
    /// Request timeout in seconds
    #[serde(default = "default_service_timeout")]
    pub timeout_seconds: u64,
    /// Retry configuration
    #[serde(default)]
    pub retry: RetryConfig,
}

fn default_service_timeout() -> u64 {
    30
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retries
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Base delay between retries in milliseconds
    #[serde(default = "default_retry_delay_ms")]
    pub base_delay_ms: u64,
    /// Maximum delay between retries in milliseconds
    #[serde(default = "default_max_retry_delay_ms")]
    pub max_delay_ms: u64,
}

fn default_max_retries() -> u32 {
    3
}

fn default_retry_delay_ms() -> u64 {
    1000
}

fn default_max_retry_delay_ms() -> u64 {
    30000
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            base_delay_ms: default_retry_delay_ms(),
            max_delay_ms: default_max_retry_delay_ms(),
        }
    }
}

/// Computer / desktop automation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputerConfig {
    /// Remote control configuration for external machines
    #[serde(default)]
    pub remote_control: RemoteControlConfig,
    /// Headless display configuration for CI/CD environments
    #[serde(default)]
    pub headless: HeadlessConfig,
    /// Enable computer use loop in agent responses
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum steps per computer use session
    #[serde(default = "default_max_computer_steps")]
    pub max_steps: usize,
    /// Settle delay after actions (milliseconds)
    #[serde(default = "default_settle_delay_ms")]
    pub settle_delay_ms: u64,
}

/// Remote control configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteControlConfig {
    /// Target host (IP or hostname)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// SSH username
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Port (22 for SSH, 5900 for VNC, 3389 for RDP)
    #[serde(default = "default_remote_port")]
    pub port: u16,
    /// Protocol: ssh, vnc, rdp
    #[serde(default = "default_remote_protocol")]
    pub protocol: String,
    /// Path to SSH private key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    /// Remote display for Linux X11 apps (e.g. ":0")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// Extra SSH arguments
    #[serde(default)]
    pub ssh_extra_args: Vec<String>,
    /// Connection timeout in seconds
    #[serde(default = "default_remote_timeout")]
    pub timeout_secs: u64,
}

/// Headless display configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadlessConfig {
    /// Enable headless mode (Xvfb/virtual display)
    #[serde(default = "default_false")]
    pub enabled: bool,
    /// Display identifier (e.g. ":99")
    #[serde(default = "default_headless_display")]
    pub display: String,
    /// Screen resolution width
    #[serde(default = "default_headless_width")]
    pub width: u32,
    /// Screen resolution height
    #[serde(default = "default_headless_height")]
    pub height: u32,
    /// Color depth
    #[serde(default = "default_headless_depth")]
    pub depth: u8,
}

fn default_max_computer_steps() -> usize {
    30
}

fn default_settle_delay_ms() -> u64 {
    500
}

fn default_remote_port() -> u16 {
    22
}

fn default_remote_protocol() -> String {
    "ssh".to_string()
}

fn default_remote_timeout() -> u64 {
    10
}

fn default_headless_display() -> String {
    ":99".to_string()
}

fn default_headless_width() -> u32 {
    1920
}

fn default_headless_height() -> u32 {
    1080
}

fn default_headless_depth() -> u8 {
    24
}

impl Default for ComputerConfig {
    fn default() -> Self {
        Self {
            remote_control: RemoteControlConfig::default(),
            headless: HeadlessConfig::default(),
            enabled: true,
            max_steps: default_max_computer_steps(),
            settle_delay_ms: default_settle_delay_ms(),
        }
    }
}

impl Default for RemoteControlConfig {
    fn default() -> Self {
        Self {
            host: None,
            user: None,
            port: default_remote_port(),
            protocol: default_remote_protocol(),
            key_path: None,
            display: Some(":0".to_string()),
            ssh_extra_args: Vec::new(),
            timeout_secs: default_remote_timeout(),
        }
    }
}

impl Default for HeadlessConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            display: default_headless_display(),
            width: default_headless_width(),
            height: default_headless_height(),
            depth: default_headless_depth(),
        }
    }
}

/// Capability set configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitiesConfig {
    /// Capability profile: minimal, observer, server, desktop, full, custom
    #[serde(default = "default_capability_profile")]
    pub profile: String,
    /// Custom set IDs when profile is "custom"
    #[serde(default)]
    pub custom_sets: Vec<String>,
    /// Maximum OsControlScope to allow (read_only, user_space, system, root)
    #[serde(default = "default_capability_max_scope")]
    pub max_scope: String,
    /// Explicitly disable specific set IDs regardless of profile
    #[serde(default)]
    pub disabled_sets: Vec<String>,
    /// Default minimum role required to invoke tools.
    #[serde(default)]
    pub default_required_role: Option<crate::tools::rbac::Role>,
    /// Default maximum tool risk level allowed.
    #[serde(default)]
    pub default_max_risk_level: Option<crate::tools::approval::RiskLevel>,
    /// Tool names denied by default across all users.
    #[serde(default)]
    pub denied_tools: Vec<String>,
    /// Tool names allowed by default (empty = all allowed).
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Tool categories allowed by default (empty = all allowed).
    #[serde(default)]
    pub allowed_categories: Vec<String>,
}

fn default_capability_profile() -> String {
    "full".to_string()
}

fn default_capability_max_scope() -> String {
    "root".to_string()
}

impl Default for CapabilitiesConfig {
    fn default() -> Self {
        Self {
            profile: default_capability_profile(),
            custom_sets: Vec::new(),
            max_scope: default_capability_max_scope(),
            disabled_sets: Vec::new(),
            default_required_role: None,
            default_max_risk_level: None,
            denied_tools: Vec::new(),
            allowed_tools: Vec::new(),
            allowed_categories: Vec::new(),
        }
    }
}

/// Browser automation configuration
#[cfg(feature = "browser")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Enable the browser bridge server
    #[serde(default = "default_false")]
    pub bridge_enabled: bool,
    /// Port for the browser bridge server
    #[serde(default = "default_bridge_port")]
    pub bridge_port: u16,
    /// Pool configuration
    #[serde(default)]
    pub pool: crate::browser::BrowserPoolConfig,
    /// Browser profiles
    #[serde(default)]
    pub profiles: Vec<crate::browser::BrowserProfile>,
}

#[cfg(feature = "browser")]
impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            bridge_enabled: false,
            bridge_port: default_bridge_port(),
            pool: crate::browser::BrowserPoolConfig::default(),
            profiles: vec![crate::browser::BrowserProfile::default()],
        }
    }
}

#[cfg(feature = "browser")]
fn default_bridge_port() -> u16 {
    18800
}
#[cfg(feature = "browser")]
fn default_false() -> bool {
    false
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            app: AppConfig::default(),
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            storage: StorageConfig::default(),
            #[cfg(feature = "browser")]
            browser: BrowserConfig::default(),
            memory: MemoryConfig::default(),
            heartbeat: crate::heartbeat::HeartbeatConfig::default(),
            computer: ComputerConfig::default(),
            standing_orders: crate::standing_orders::config::StandingOrderConfig::default(),
            capabilities: CapabilitiesConfig::default(),
            services: HashMap::new(),
            extra: HashMap::new(),
        }
    }
}

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

        // Storage: database type requires a connection string
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

/// Configuration change callback
pub type ConfigChangeCallback = Box<dyn Fn(&Config) + Send + Sync>;

/// Configuration watcher for hot-reloading
pub struct ConfigWatcher {
    _watcher: Box<dyn std::any::Any + Send + Sync>,
    _change_tx: tokio::sync::mpsc::Sender<()>,
}

impl ConfigWatcher {
    /// Start watching a config file for changes
    pub fn watch<P: AsRef<Path>>(
        path: P,
        config_path: P,
        on_change: ConfigChangeCallback,
    ) -> crate::Result<(Self, tokio::sync::mpsc::Receiver<()>)> {
        let (change_tx, change_rx) = tokio::sync::mpsc::channel(10);
        let path = path.as_ref().to_path_buf();
        let config_path_for_reload = config_path.as_ref().to_path_buf();

        let change_tx_clone = change_tx.clone();
        let mut watcher: notify::RecommendedWatcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                match res {
                    Ok(event) => {
                        // Only react to modify/create events
                        if matches!(
                            event.kind,
                            notify::EventKind::Modify(_) | notify::EventKind::Create(_)
                        ) {
                            // Try to reload config
                            match Config::load_with_file(Some(&config_path_for_reload)) {
                                Ok(new_config) => {
                                    tracing::info!("Configuration reloaded successfully");
                                    on_change(&new_config);
                                    let _ = change_tx_clone.try_send(());
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to reload configuration: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Config file watcher error: {}", e);
                    }
                }
            })
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!(
                    "Failed to create config watcher: {}",
                    e
                ))
            })?;

        notify::Watcher::watch(&mut watcher, &path, notify::RecursiveMode::NonRecursive).map_err(
            |e| {
                crate::error::SyscityError::Internal(format!(
                    "Failed to watch config file {}: {}",
                    path.display(),
                    e
                ))
            },
        )?;

        Ok((
            ConfigWatcher {
                _watcher: Box::new(watcher),
                _change_tx: change_tx,
            },
            change_rx,
        ))
    }
}

/// Reloadable configuration handle
pub struct ReloadableConfig {
    config: std::sync::Arc<tokio::sync::RwLock<Config>>,
    _watcher: ConfigWatcher,
    config_tx: tokio::sync::broadcast::Sender<Config>,
}

impl ReloadableConfig {
    /// Create a new reloadable config
    pub async fn new(config: Config) -> crate::Result<Self> {
        let config_path =
            Self::find_config_file_path().unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILE));

        let config_arc = std::sync::Arc::new(tokio::sync::RwLock::new(config));
        let config_for_callback = config_arc.clone();

        let (config_tx, _) = tokio::sync::broadcast::channel::<Config>(16);
        let tx_for_callback = config_tx.clone();

        let (watcher, mut _change_rx) = ConfigWatcher::watch(
            &config_path,
            &config_path,
            Box::new(move |new_config: &Config| {
                let rt = tokio::runtime::Handle::current();
                let config = config_for_callback.clone();
                let tx = tx_for_callback.clone();
                let new_config = new_config.clone();
                rt.spawn(async move {
                    let mut guard = config.write().await;
                    *guard = new_config.clone();
                    let _ = tx.send(new_config);
                });
            }),
        )?;

        Ok(ReloadableConfig {
            config: config_arc,
            _watcher: watcher,
            config_tx,
        })
    }

    /// Get the current configuration
    pub async fn get(&self) -> Config {
        self.config.read().await.clone()
    }

    /// Subscribe to configuration changes.
    ///
    /// Returns a [`tokio::sync::broadcast::Receiver`] that receives the new
    /// [`Config`] each time the config file is reloaded.  The channel has a
    /// capacity of 16 messages; slow consumers may lag behind if they do not
    /// drain the receiver promptly.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Config> {
        self.config_tx.subscribe()
    }

    /// Find the config file path
    fn find_config_file_path() -> Option<PathBuf> {
        let candidates = [
            PathBuf::from(DEFAULT_CONFIG_FILE),
            PathBuf::from(format!(".config/{}=", DEFAULT_CONFIG_FILE)),
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
}

pub mod hot_reload {
    //! Hot Config Reload System
    //!
    //! Watches configuration files for changes and reloads them at runtime
    //! without requiring a restart.

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    #[cfg(feature = "hot-reload")]
    use notify::{RecommendedWatcher, RecursiveMode};
    #[cfg(feature = "hot-reload")]
    use notify_debouncer_full::{new_debouncer, DebouncedEvent, Debouncer, FileIdMap};
    use tokio::sync::{mpsc, RwLock};
    use tracing::{debug, error, info, warn};

    /// Configuration file types that can be hot-reloaded
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum ConfigFileType {
        /// Main application configuration
        Main,
        /// Agent configuration
        Agent,
        /// Channel configuration
        Channel,
        /// Plugin configuration
        Plugin,
        /// Gateway configuration
        Gateway,
        /// Custom config file
        Custom,
    }

    /// A watched configuration file
    #[derive(Debug, Clone)]
    pub struct WatchedConfig {
        /// File path
        pub path: PathBuf,
        /// Config type
        pub config_type: ConfigFileType,
        /// Whether the file is currently valid
        pub is_valid: bool,
    }

    /// Configuration change event
    #[derive(Debug, Clone)]
    pub struct ConfigChangeEvent {
        /// Path of the changed file
        pub path: PathBuf,
        /// Config type
        pub config_type: ConfigFileType,
        /// Type of change
        pub change_type: ConfigChangeType,
    }

    /// Type of configuration change
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ConfigChangeType {
        /// File was created
        Created,
        /// File was modified
        Modified,
        /// File was deleted
        Deleted,
        /// File was renamed
        Renamed,
    }

    /// Handler function for config changes
    type ConfigChangeHandler = Arc<
        dyn Fn(ConfigChangeEvent) -> futures::future::BoxFuture<'static, Result<(), String>>
            + Send
            + Sync,
    >;

    /// Hot reload manager
    pub struct HotReloadManager {
        /// Watched files
        watched_files: Arc<RwLock<HashMap<PathBuf, WatchedConfig>>>,
        /// Registered handlers
        handlers: Arc<RwLock<HashMap<ConfigFileType, Vec<ConfigChangeHandler>>>>,
        /// Channel for change events
        change_tx: mpsc::Sender<ConfigChangeEvent>,
        change_rx: Arc<RwLock<mpsc::Receiver<ConfigChangeEvent>>>,
        /// File watcher (only available with hot-reload feature)
        #[cfg(feature = "hot-reload")]
        watcher: Arc<RwLock<Option<Debouncer<RecommendedWatcher, FileIdMap>>>>,
    }

    impl HotReloadManager {
        /// Create a new hot reload manager
        pub fn new() -> crate::Result<Self> {
            let (change_tx, change_rx) = mpsc::channel(100);

            Ok(Self {
                watched_files: Arc::new(RwLock::new(HashMap::new())),
                handlers: Arc::new(RwLock::new(HashMap::new())),
                change_tx,
                change_rx: Arc::new(RwLock::new(change_rx)),
                #[cfg(feature = "hot-reload")]
                watcher: Arc::new(RwLock::new(None)),
            })
        }

        /// Start watching files
        #[cfg(feature = "hot-reload")]
        pub async fn start(&self) -> crate::Result<()> {
            let change_tx = self.change_tx.clone();
            let watched_files = self.watched_files.clone();

            // Create debouncer with 500ms delay
            let debouncer = new_debouncer(
                Duration::from_millis(500),
                None,
                move |result: Result<Vec<DebouncedEvent>, Vec<notify::Error>>| {
                    match result {
                        Ok(events) => {
                            for debounced_event in events {
                                let notify_event = &debounced_event.event;
                                let change_type = match notify_event.kind {
                                    notify::EventKind::Create(_) => ConfigChangeType::Created,
                                    notify::EventKind::Modify(_) => ConfigChangeType::Modified,
                                    notify::EventKind::Remove(_) => ConfigChangeType::Deleted,
                                    _ => continue,
                                };

                                // Process each path in the event
                                for path in &notify_event.paths {
                                    // Look up the config type for this path
                                    let config_type = {
                                        let files = futures::executor::block_on(async {
                                            watched_files.read().await
                                        });
                                        files
                                            .get(path)
                                            .map(|f| f.config_type)
                                            .unwrap_or(ConfigFileType::Custom)
                                    };

                                    let event = ConfigChangeEvent {
                                        path: path.clone(),
                                        config_type,
                                        change_type,
                                    };

                                    if let Err(e) = change_tx.try_send(event) {
                                        warn!("Failed to send config change event: {}", e);
                                    } else {
                                        info!("Config file changed: {:?}", path);
                                    }
                                }
                            }
                        }
                        Err(errors) => {
                            for e in errors {
                                error!("File watcher error: {}", e);
                            }
                        }
                    }
                },
            )
            .map_err(|e| crate::error::ConfigError::InvalidValue {
                key: "hot_reload".to_string(),
                message: format!("Failed to create file watcher: {}", e),
            })?;

            // Store the watcher
            {
                let mut watcher_guard = self.watcher.write().await;
                *watcher_guard = Some(debouncer);
            }

            info!("Hot reload manager started with file watching enabled");
            Ok(())
        }

        /// Start watching files (without hot-reload feature)
        #[cfg(not(feature = "hot-reload"))]
        pub async fn start(&self) -> crate::Result<()> {
            info!("Hot reload manager started (file watching disabled without hot-reload feature)");
            Ok(())
        }

        /// Watch a configuration file
        pub async fn watch_file(
            &self,
            path: impl AsRef<Path>,
            config_type: ConfigFileType,
        ) -> crate::Result<()> {
            let path = path.as_ref().to_path_buf();

            // Check if file exists
            if !path.exists() {
                warn!("Cannot watch non-existent file: {:?}", path);
                return Err(crate::error::ConfigError::Missing(format!(
                    "File not found: {:?}",
                    path
                ))
                .into());
            }

            // Add to watched files
            let watched_config = WatchedConfig {
                path: path.clone(),
                config_type,
                is_valid: true,
            };

            {
                let mut files = self.watched_files.write().await;
                files.insert(path.clone(), watched_config);
            }

            // Register with file watcher if hot-reload feature is enabled
            #[cfg(feature = "hot-reload")]
            {
                let mut watcher_guard = self.watcher.write().await;
                if let Some(ref mut debouncer) = *watcher_guard {
                    use notify::Watcher;
                    if let Err(e) = debouncer
                        .watcher()
                        .watch(&path, RecursiveMode::NonRecursive)
                    {
                        warn!("Failed to add file to watcher: {:?} - {}", path, e);
                    } else {
                        debug!("Added file to notify watcher: {:?}", path);
                    }
                }
            }

            info!("Watching config file: {:?} ({:?})", path, config_type);
            Ok(())
        }

        /// Unwatch a file
        pub async fn unwatch_file(&self, path: impl AsRef<Path>) -> crate::Result<bool> {
            let path = path.as_ref();
            let mut files = self.watched_files.write().await;

            if files.remove(path).is_some() {
                info!("Stopped watching config file: {:?}", path);
                Ok(true)
            } else {
                Ok(false)
            }
        }

        /// Register a handler for config changes
        pub async fn register_handler<F, Fut>(&self, config_type: ConfigFileType, handler: F)
        where
            F: Fn(ConfigChangeEvent) -> Fut + Send + Sync + 'static,
            Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
        {
            let mut handlers = self.handlers.write().await;
            let handler_list = handlers.entry(config_type).or_default();

            handler_list.push(Arc::new(move |event| Box::pin(handler(event))));

            debug!("Registered handler for {:?}", config_type);
        }

        /// Process configuration changes
        pub async fn run(&self) -> crate::Result<()> {
            let mut rx = self.change_rx.write().await;

            while let Some(event) = rx.recv().await {
                info!("Processing config change: {:?} ({:?})", event.path, event.change_type);

                // Get handlers for this config type
                let handlers = {
                    let handlers = self.handlers.read().await;
                    handlers.get(&event.config_type).cloned()
                };

                if let Some(handlers) = handlers {
                    for handler in handlers {
                        match handler(event.clone()).await {
                            Ok(_) => {
                                debug!("Handler succeeded for {:?}", event.path);
                            }
                            Err(e) => {
                                error!("Handler failed for {:?}: {}", event.path, e);
                            }
                        }
                    }
                }
            }

            Ok(())
        }

        /// Stop watching files
        pub async fn stop(&self) -> crate::Result<()> {
            let mut files = self.watched_files.write().await;
            files.clear();

            #[cfg(feature = "hot-reload")]
            {
                let mut watcher = self.watcher.write().await;
                *watcher = None;
            }

            info!("Hot reload manager stopped");
            Ok(())
        }

        /// List all watched files
        pub async fn list_watched(&self) -> Vec<WatchedConfig> {
            let files = self.watched_files.read().await;
            files.values().cloned().collect()
        }

        /// Check if a file is being watched
        pub async fn is_watched(&self, path: impl AsRef<Path>) -> bool {
            let files = self.watched_files.read().await;
            files.contains_key(path.as_ref())
        }
    }

    impl Default for HotReloadManager {
        fn default() -> Self {
            #[allow(clippy::expect_used)]
            Self::new().expect("Failed to create HotReloadManager")
        }
    }

    /// Builder for hot reload setup
    pub struct HotReloadBuilder {
        config_paths: Vec<(PathBuf, ConfigFileType)>,
    }

    impl HotReloadBuilder {
        /// Create a new builder
        pub fn new() -> Self {
            Self { config_paths: vec![] }
        }

        /// Add a config file to watch
        pub fn watch(mut self, path: impl AsRef<Path>, config_type: ConfigFileType) -> Self {
            self.config_paths
                .push((path.as_ref().to_path_buf(), config_type));
            self
        }

        /// Build and start the hot reload manager
        pub async fn build(self) -> crate::Result<HotReloadManager> {
            let manager = HotReloadManager::new()?;
            manager.start().await?;

            for (path, config_type) in self.config_paths {
                if let Err(e) = manager.watch_file(&path, config_type).await {
                    warn!("Failed to watch {:?}: {}", path, e);
                }
            }

            Ok(manager)
        }
    }

    impl Default for HotReloadBuilder {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn test_schema_version_from_toml() {
        let toml_str = r#"
schema_version = 0

[server]
host = "0.0.0.0"
port = 3000
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.schema_version, 0);
    }

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
    fn test_load_from_toml() {
        let toml_str = r#"
[server]
host = "0.0.0.0"
port = 3000

[logging]
level = "debug"
format = "json"
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.logging.level, "debug");
        match config.logging.format {
            LogFormat::Json => {}
            _ => panic!("Expected JSON format"),
        }
    }

    #[test]
    fn test_service_config() {
        let toml_str = r#"
[services.api]
endpoint = "https://api.example.com"
api_key = "secret123"
timeout_seconds = 60

[services.api.retry]
max_retries = 5
base_delay_ms = 500
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let api_service = config.get_service("api").unwrap();
        assert_eq!(api_service.endpoint, "https://api.example.com");
        assert_eq!(api_service.api_key, Some(SecretRef::String("secret123".to_string())));
        assert_eq!(api_service.timeout_seconds, 60);
        assert_eq!(api_service.retry.max_retries, 5);
        assert_eq!(api_service.retry.base_delay_ms, 500);
    }

    #[test]
    fn test_service_config_with_env_ref() {
        let toml_str = r#"
[services.api]
endpoint = "https://api.example.com"
api_key = "$API_KEY"
timeout_seconds = 60
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let api_service = config.get_service("api").unwrap();
        assert_eq!(api_service.api_key, Some(SecretRef::String("$API_KEY".to_string())));
    }

    #[test]
    fn test_service_config_with_explicit_env() {
        let toml_str = r#"
[services.api]
endpoint = "https://api.example.com"
api_key = { env = "API_KEY" }
timeout_seconds = 60
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let api_service = config.get_service("api").unwrap();
        assert_eq!(
            api_service.api_key,
            Some(SecretRef::Explicit {
                env: Some("API_KEY".to_string()),
                file: None,
                exec: None,
            })
        );
    }

    #[test]
    fn test_service_config_with_file_ref() {
        let toml_str = r#"
[services.api]
endpoint = "https://api.example.com"
api_key = { file = "/run/secrets/api_key" }
timeout_seconds = 60
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let api_service = config.get_service("api").unwrap();
        assert_eq!(
            api_service.api_key,
            Some(SecretRef::Explicit {
                env: None,
                file: Some(std::path::PathBuf::from("/run/secrets/api_key")),
                exec: None,
            })
        );
    }

    #[test]
    #[cfg(feature = "browser")]
    fn test_browser_config_default() {
        let config = Config::default();
        assert!(!config.browser.bridge_enabled);
        assert_eq!(config.browser.bridge_port, 18800);
        assert_eq!(config.browser.pool.default_profile, "default");
        assert_eq!(config.browser.profiles.len(), 1);
    }

    #[test]
    #[cfg(feature = "browser")]
    fn test_browser_config_from_toml() {
        let toml_str = r#"
[browser]
bridge_enabled = true
bridge_port = 18801

[browser.pool]
idle_timeout_secs = 600
cleanup_interval_secs = 120

[[browser.profiles]]
name = "default"
headless = true
viewport_width = 1280
viewport_height = 720

[[browser.profiles]]
name = "headed"
headless = false
viewport_width = 1920
viewport_height = 1080
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.browser.bridge_enabled);
        assert_eq!(config.browser.bridge_port, 18801);
        assert_eq!(config.browser.pool.idle_timeout_secs, 600);
        assert_eq!(config.browser.pool.cleanup_interval_secs, 120);
        assert_eq!(config.browser.profiles.len(), 2);
        assert_eq!(config.browser.profiles[0].name, "default");
        assert!(config.browser.profiles[0].headless);
        assert_eq!(config.browser.profiles[1].name, "headed");
        assert!(!config.browser.profiles[1].headless);
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
    fn test_computer_config_default() {
        let config = Config::default();
        assert!(config.computer.enabled);
        assert_eq!(config.computer.max_steps, 30);
        assert_eq!(config.computer.settle_delay_ms, 500);
        assert!(!config.computer.headless.enabled);
        assert_eq!(config.computer.headless.display, ":99");
        assert_eq!(config.computer.remote_control.port, 22);
        assert_eq!(config.computer.remote_control.protocol, "ssh");
    }

    #[test]
    fn test_computer_config_from_toml() {
        let toml_str = r#"
[computer]
enabled = true
max_steps = 50

[computer.remote_control]
host = "192.168.1.100"
user = "admin"
port = 2222
protocol = "ssh"
key_path = "~/.ssh/id_rsa"
display = ":1"

[computer.headless]
enabled = true
display = ":99"
width = 1280
height = 720
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.computer.enabled);
        assert_eq!(config.computer.max_steps, 50);
        assert_eq!(config.computer.remote_control.host, Some("192.168.1.100".to_string()));
        assert_eq!(config.computer.remote_control.user, Some("admin".to_string()));
        assert_eq!(config.computer.remote_control.port, 2222);
        assert_eq!(config.computer.remote_control.protocol, "ssh");
        assert_eq!(config.computer.remote_control.key_path, Some("~/.ssh/id_rsa".to_string()));
        assert_eq!(config.computer.remote_control.display, Some(":1".to_string()));
        assert!(config.computer.headless.enabled);
        assert_eq!(config.computer.headless.display, ":99");
        assert_eq!(config.computer.headless.width, 1280);
        assert_eq!(config.computer.headless.height, 720);
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

    #[test]
    fn test_standing_orders_config_default() {
        let config = Config::default();
        assert!(config.standing_orders.enabled);
        assert!(config.standing_orders.orders.is_empty());
    }

    #[test]
    fn test_standing_orders_config_from_toml() {
        let toml_str = r#"
[standing_orders]
enabled = true

[[standing_orders.orders]]
name = "daily_summary"
description = "Send daily summary to team channel"
agent_id = "assistant"
schedule = "0 0 9 * * *"
prompt = "Generate a summary of today's key events and priorities."
output_channel = "slack_general"
enabled = true

[[standing_orders.orders]]
name = "hourly_check"
agent_id = "monitor"
schedule = "0 * * * * *"
prompt = "Check system health and report anomalies."
enabled = false
timeout_secs = 30
"#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let so = &config.standing_orders;
        assert!(so.enabled);
        assert_eq!(so.orders.len(), 2);

        let first = &so.orders[0];
        assert_eq!(first.name, "daily_summary");
        assert_eq!(first.description.as_deref(), Some("Send daily summary to team channel"));
        assert_eq!(first.agent_id, "assistant");
        assert_eq!(first.schedule, "0 0 9 * * *");
        assert_eq!(first.prompt, "Generate a summary of today's key events and priorities.");
        assert_eq!(first.output_channel.as_deref(), Some("slack_general"));
        assert!(first.enabled);
        assert!(first.timeout_secs.is_none());

        let second = &so.orders[1];
        assert_eq!(second.name, "hourly_check");
        assert!(!second.enabled);
        assert_eq!(second.timeout_secs, Some(30));
    }
}
