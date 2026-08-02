//! Configuration watcher and reloadable handle.

use std::path::{Path, PathBuf};

use super::types::DEFAULT_CONFIG_FILE;
use super::Config;

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
