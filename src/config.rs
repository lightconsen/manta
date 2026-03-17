//! Configuration management for Manta
//!
//! This module handles loading and validating configuration from
//! multiple sources: defaults, config files, and environment variables.

use crate::error::{ConfigError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Default configuration file name
pub const DEFAULT_CONFIG_FILE: &str = "manta.toml";

/// Environment variable prefix
pub const ENV_PREFIX: &str = "MANTA";

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
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

/// External service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Service endpoint URL
    pub endpoint: String,
    /// API key (can reference env var with ${ENV_VAR} syntax)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
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

impl Default for Config {
    fn default() -> Self {
        Self {
            app: AppConfig::default(),
            server: ServerConfig::default(),
            logging: LoggingConfig::default(),
            storage: StorageConfig::default(),
            services: HashMap::new(),
            extra: HashMap::new(),
        }
    }
}

impl Config {
    /// Load configuration from default sources
    ///
    /// The configuration is loaded in the following order (later sources override earlier ones):
    /// 1. Default values
    /// 2. Config file (manta.toml or specified path)
    /// 3. Environment variables (MANTA_*)
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
            .or_else(|| Self::find_config_file());

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
            // Centralized ~/.manta/manta.toml
            crate::dirs::default_config_file(),
            // Legacy location for backwards compatibility
            dirs::config_dir()
                .map(|d| d.join("manta").join(DEFAULT_CONFIG_FILE))
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
            self.server.port = port.parse().map_err(|e| {
                ConfigError::InvalidValue {
                    key: "server.port".to_string(),
                    message: format!("Invalid port number: {}", e),
                }
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

        Ok(())
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
        let mut watcher: notify::RecommendedWatcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
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
            crate::error::MantaError::Internal(format!("Failed to create config watcher: {}", e))
        })?;

        notify::Watcher::watch(&mut watcher, &path, notify::RecursiveMode::NonRecursive)
            .map_err(|e| {
                crate::error::MantaError::Internal(format!(
                    "Failed to watch config file {}: {}",
                    path.display(),
                    e
                ))
            })?;

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
}

impl ReloadableConfig {
    /// Create a new reloadable config
    pub async fn new(config: Config) -> crate::Result<Self> {
        let config_path = Self::find_config_file_path()
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILE));

        let config_arc = std::sync::Arc::new(tokio::sync::RwLock::new(config));
        let config_for_callback = config_arc.clone();

        let (watcher, mut _change_rx) = ConfigWatcher::watch(
            &config_path,
            &config_path,
            Box::new(move |new_config: &Config| {
                // Update the shared config
                let rt = tokio::runtime::Handle::current();
                let config = config_for_callback.clone();
                let new_config = new_config.clone();
                rt.spawn(async move {
                    let mut guard = config.write().await;
                    *guard = new_config;
                });
            }),
        )?;

        Ok(ReloadableConfig {
            config: config_arc,
            _watcher: watcher,
        })
    }

    /// Get the current configuration
    pub async fn get(&self) -> Config {
        self.config.read().await.clone()
    }

    /// Find the config file path
    fn find_config_file_path() -> Option<PathBuf> {
        let candidates = [
            PathBuf::from(DEFAULT_CONFIG_FILE),
            PathBuf::from(format!(".config/{}=", DEFAULT_CONFIG_FILE)),
            dirs::config_dir()
                .map(|d| d.join("manta").join(DEFAULT_CONFIG_FILE))
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

/// Hot reload module for runtime configuration reloading
pub mod hot_reload {
    //! Hot Config Reload System
    //!
    //! Watches configuration files for changes and reloads them at runtime
    //! without requiring a restart.

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{mpsc, RwLock};
    use tracing::{debug, error, info, warn};

    #[cfg(feature = "hot-reload")]
    use notify_debouncer_full::{new_debouncer, DebouncedEvent, Debouncer, FileIdMap};
    #[cfg(feature = "hot-reload")]
    use notify::{RecursiveMode, RecommendedWatcher};

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
                                        files.get(path).map(|f| f.config_type)
                                            .unwrap_or(ConfigFileType::Custom)
                                    };

                                    let event = ConfigChangeEvent {
                                        path: path.clone(),
                                        config_type,
                                        change_type: change_type.clone(),
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
        pub async fn register_handler<F, Fut>(
            &self,
            config_type: ConfigFileType,
            handler: F,
        ) where
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
                info!(
                    "Processing config change: {:?} ({:?})",
                    event.path, event.change_type
                );

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
            Self {
                config_paths: vec![],
            }
        }

        /// Add a config file to watch
        pub fn watch(mut self, path: impl AsRef<Path>, config_type: ConfigFileType) -> Self {
            self.config_paths.push((path.as_ref().to_path_buf(), config_type));
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
            LogFormat::Json => {},
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
        assert_eq!(api_service.api_key, Some("secret123".to_string()));
        assert_eq!(api_service.timeout_seconds, 60);
        assert_eq!(api_service.retry.max_retries, 5);
        assert_eq!(api_service.retry.base_delay_ms, 500);
    }
}
