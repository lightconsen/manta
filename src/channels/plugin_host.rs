//! WASM Plugin Host for Channel Extensions
//!
//! This module provides the runtime for loading and executing WASM-based
//! channel plugins, enabling third-party channels without recompiling Manta.

use crate::channels::{Channel, ChannelCapabilities, ConversationId, Id, IncomingMessage, OutgoingMessage};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};
use wasmtime::{Engine, Linker, Module, Store, TypedFunc};
use wasmtime_wasi::WasiCtxBuilder;

/// Host state passed to WASM plugins
pub struct HostState {
    /// Channel name
    pub name: String,
    /// Configuration JSON
    pub config: String,
    /// Message sender for incoming messages
    pub message_tx: mpsc::UnboundedSender<IncomingMessage>,
    /// Logger
    pub log_tx: mpsc::UnboundedSender<(LogLevel, String)>,
}

/// Log levels for plugin logging
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// A WASM-based channel plugin
pub struct PluginChannel {
    /// Plugin name
    name: String,
    /// Plugin ID
    id: String,
    /// WASM store - wrapped in Mutex for thread safety
    store: Arc<Mutex<Store<HostState>>>,
    /// Function pointers - stored as Option to allow re-initialization
    init_fn: TypedFunc<(String,), (Result<String, String>,)>,
    start_fn: TypedFunc<(), (Result<(), String>,)>,
    stop_fn: TypedFunc<(), (Result<(), String>,)>,
    get_name_fn: TypedFunc<(), (String,)>,
    get_capabilities_fn: TypedFunc<(), (PluginCapabilities,)>,
    send_fn: TypedFunc<(PluginOutgoingMessage, PluginMessageOptions), (Result<String, String>,)>,
    send_typing_fn: TypedFunc<(String,), (Result<(), String>,)>,
    edit_message_fn: TypedFunc<(String, String), (Result<(), String>,)>,
    delete_message_fn: TypedFunc<(String,), (Result<(), String>,)>,
    health_check_fn: TypedFunc<(), (Result<bool, String>,)>,
}

/// Capabilities as represented in WASM
#[derive(Clone, Debug)]
pub struct PluginCapabilities {
    pub supports_formatting: bool,
    pub supports_attachments: bool,
    pub supports_images: bool,
    pub supports_threads: bool,
    pub supports_typing: bool,
    pub supports_buttons: bool,
    pub supports_commands: bool,
    pub supports_reactions: bool,
}

/// Outgoing message for WASM interface
#[derive(Clone, Debug)]
pub struct PluginOutgoingMessage {
    pub conversation_id: String,
    pub content: String,
    pub formatted_content: Option<String>,
    pub reply_to: Option<String>,
}

/// Message options for WASM interface
#[derive(Clone, Debug)]
pub struct PluginMessageOptions {
    pub show_typing: bool,
    pub silent: bool,
}

impl PluginChannel {
    /// Load a WASM plugin from file
    pub async fn load(
        wasm_path: &std::path::Path,
        config: serde_json::Value,
        message_tx: mpsc::UnboundedSender<IncomingMessage>,
    ) -> crate::Result<Self> {
        debug!("Loading WASM plugin from {:?}", wasm_path);

        // Read WASM bytes
        let wasm_bytes = tokio::fs::read(wasm_path).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: format!("Failed to read WASM file: {:?}", wasm_path),
                details: e.to_string(),
            }
        })?;

        // Create engine (run synchronously in blocking task)
        let (store, init_fn, start_fn, stop_fn, get_name_fn, get_capabilities_fn,
             send_fn, send_typing_fn, edit_message_fn, delete_message_fn, health_check_fn) =
            tokio::task::spawn_blocking(move || {
                let engine = Engine::default();

                // Compile module
                let module = Module::new(&engine, &wasm_bytes).map_err(|e| {
                    crate::error::MantaError::Plugin(format!("Failed to compile WASM module: {}", e))
                })?;

                // Create linker
                let mut linker: Linker<HostState> = Linker::new(&engine);

                // Add WASI support
                wasmtime_wasi::add_to_linker(&mut linker, |_state: &mut HostState| {
                    WasiCtxBuilder::new()
                        .inherit_stdio()
                        .build()
                }).map_err(|e| {
                    crate::error::MantaError::Plugin(format!("Failed to add WASI: {}", e))
                })?;

                // Create host state
                let (log_tx, mut log_rx) = mpsc::unbounded_channel();

                let host_state = HostState {
                    name: "plugin".to_string(),
                    config: config.to_string(),
                    message_tx,
                    log_tx,
                };

                // Create store
                let mut store = Store::new(&engine, host_state);

                // Create instance
                let instance = linker.instantiate(&mut store, &module).map_err(|e| {
                    crate::error::MantaError::Plugin(format!("Failed to instantiate WASM: {}", e))
                })?;

                // Get function pointers
                let init_fn = instance
                    .get_typed_func::<(String,), (Result<String, String>,)>(&mut store, "init")
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'init' export: {}", e))
                    })?;

                let start_fn = instance
                    .get_typed_func::<(), (Result<(), String>,)>(&mut store, "start")
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'start' export: {}", e))
                    })?;

                let stop_fn = instance
                    .get_typed_func::<(), (Result<(), String>,)>(&mut store, "stop")
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'stop' export: {}", e))
                    })?;

                let get_name_fn = instance
                    .get_typed_func::<(), (String,)>(&mut store, "get_name")
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'get_name' export: {}", e))
                    })?;

                let get_capabilities_fn = instance
                    .get_typed_func::<(), (PluginCapabilities,)>(&mut store, "get_capabilities")
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'get_capabilities' export: {}", e))
                    })?;

                let send_fn = instance
                    .get_typed_func::<(PluginOutgoingMessage, PluginMessageOptions), (Result<String, String>,)>(
                        &mut store, "send",
                    )
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'send' export: {}", e))
                    })?;

                let send_typing_fn = instance
                    .get_typed_func::<(String,), (Result<(), String>,)>(&mut store, "send_typing")
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'send_typing' export: {}", e))
                    })?;

                let edit_message_fn = instance
                    .get_typed_func::<(String, String), (Result<(), String>,)>(&mut store, "edit_message")
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'edit_message' export: {}", e))
                    })?;

                let delete_message_fn = instance
                    .get_typed_func::<(String,), (Result<(), String>,)>(&mut store, "delete_message")
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'delete_message' export: {}", e))
                    })?;

                let health_check_fn = instance
                    .get_typed_func::<(), (Result<bool, String>,)>(&mut store, "health_check")
                    .map_err(|e| {
                        crate::error::MantaError::Plugin(format!("Missing 'health_check' export: {}", e))
                    })?;

                Ok::<_, crate::error::MantaError>((
                    store, init_fn, start_fn, stop_fn, get_name_fn, get_capabilities_fn,
                    send_fn, send_typing_fn, edit_message_fn, delete_message_fn, health_check_fn
                ))
            }).await.map_err(|e| {
                crate::error::MantaError::Plugin(format!("Blocking task failed: {}", e))
            })??;

        let plugin_name = wasm_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let plugin_id = uuid::Uuid::new_v4().to_string();

        info!("Loaded WASM plugin '{}' (ID: {})", plugin_name, plugin_id);

        Ok(Self {
            name: plugin_name,
            id: plugin_id,
            store: Arc::new(Mutex::new(store)),
            init_fn,
            start_fn,
            stop_fn,
            get_name_fn,
            get_capabilities_fn,
            send_fn,
            send_typing_fn,
            edit_message_fn,
            delete_message_fn,
            health_check_fn,
        })
    }

    /// Initialize the plugin with configuration
    pub async fn init(&self, config: &serde_json::Value) -> crate::Result<String> {
        let config_str = config.to_string();
        let init_fn = self.init_fn;
        let store = self.store.clone();

        let result = tokio::task::spawn_blocking(move || {
            let mut store = store.blocking_lock();
            init_fn.call(&mut *store, (config_str,))
        }).await.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Init task failed: {}", e))
        })?;

        result.map_err(|e| crate::error::MantaError::Plugin(format!("Plugin init failed: {}", e)))
    }
}

#[async_trait]
impl Channel for PluginChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            supports_formatting: true,
            supports_attachments: true,
            supports_images: true,
            supports_threads: true,
            supports_typing: true,
            supports_buttons: false,
            supports_commands: false,
            supports_reactions: false,
        }
    }

    async fn start(&self) -> crate::Result<()> {
        let start_fn = self.start_fn;
        let store = self.store.clone();

        let (result,) = tokio::task::spawn_blocking(move || {
            let mut store = store.blocking_lock();
            start_fn.call(&mut *store, ())
        }).await.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Start task failed: {}", e))
        })?;

        result.map_err(|e| crate::error::MantaError::Plugin(format!("Plugin start failed: {}", e)))?;

        info!("Started WASM plugin channel '{}'", self.name);
        Ok(())
    }

    async fn stop(&self) -> crate::Result<()> {
        let stop_fn = self.stop_fn;
        let store = self.store.clone();

        let (result,) = tokio::task::spawn_blocking(move || {
            let mut store = store.blocking_lock();
            stop_fn.call(&mut *store, ())
        }).await.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Stop task failed: {}", e))
        })?;

        result.map_err(|e| crate::error::MantaError::Plugin(format!("Plugin stop failed: {}", e)))?;

        info!("Stopped WASM plugin channel '{}'", self.name);
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id> {
        let plugin_msg = PluginOutgoingMessage {
            conversation_id: message.conversation_id.to_string(),
            content: message.content,
            formatted_content: None,
            reply_to: message.reply_to.map(|id| id.to_string()),
        };

        let options = PluginMessageOptions {
            show_typing: message.options.show_typing,
            silent: message.options.silent,
        };

        let send_fn = self.send_fn;
        let store = self.store.clone();

        let (result,) = tokio::task::spawn_blocking(move || {
            let mut store = store.blocking_lock();
            send_fn.call(&mut *store, (plugin_msg, options))
        }).await.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Send task failed: {}", e))
        })?;

        let message_id = result.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Plugin send failed: {}", e))
        })?;

        Ok(Id(message_id))
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> crate::Result<()> {
        let send_typing_fn = self.send_typing_fn;
        let store = self.store.clone();
        let conv_id = conversation_id.to_string();

        let (result,) = tokio::task::spawn_blocking(move || {
            let mut store = store.blocking_lock();
            send_typing_fn.call(&mut *store, (conv_id,))
        }).await.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Send typing task failed: {}", e))
        })?;

        result.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Plugin send_typing failed: {}", e))
        })
    }

    async fn edit_message(&self, message_id: Id, new_content: String) -> crate::Result<()> {
        let edit_message_fn = self.edit_message_fn;
        let store = self.store.clone();
        let msg_id = message_id.to_string();

        let (result,) = tokio::task::spawn_blocking(move || {
            let mut store = store.blocking_lock();
            edit_message_fn.call(&mut *store, (msg_id, new_content))
        }).await.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Edit message task failed: {}", e))
        })?;

        result.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Plugin edit_message failed: {}", e))
        })
    }

    async fn delete_message(&self, message_id: Id) -> crate::Result<()> {
        let delete_message_fn = self.delete_message_fn;
        let store = self.store.clone();
        let msg_id = message_id.to_string();

        let (result,) = tokio::task::spawn_blocking(move || {
            let mut store = store.blocking_lock();
            delete_message_fn.call(&mut *store, (msg_id,))
        }).await.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Delete message task failed: {}", e))
        })?;

        result.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Plugin delete_message failed: {}", e))
        })
    }

    async fn health_check(&self) -> crate::Result<bool> {
        let health_check_fn = self.health_check_fn;
        let store = self.store.clone();

        let (result,) = tokio::task::spawn_blocking(move || {
            let mut store = store.blocking_lock();
            health_check_fn.call(&mut *store, ())
        }).await.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Health check task failed: {}", e))
        })?;

        result.map_err(|e| {
            crate::error::MantaError::Plugin(format!("Plugin health_check failed: {}", e))
        })
    }
}

/// Plugin manifest
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub config_schema: Option<serde_json::Value>,
}

/// Registry for managing WASM channel plugins
pub struct PluginChannelRegistry {
    /// Loaded plugins
    plugins: Arc<RwLock<HashMap<String, Arc<PluginChannel>>>>,
    /// Plugin directory
    plugin_dir: PathBuf,
    /// Message sender for incoming messages
    message_tx: mpsc::UnboundedSender<IncomingMessage>,
}

impl PluginChannelRegistry {
    /// Create a new plugin registry
    pub fn new(
        plugin_dir: PathBuf,
        message_tx: mpsc::UnboundedSender<IncomingMessage>,
    ) -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugin_dir,
            message_tx,
        }
    }

    /// Discover available plugins
    pub async fn discover_plugins(&self) -> crate::Result<Vec<(String, PathBuf)>> {
        let mut plugins = Vec::new();

        if !self.plugin_dir.exists() {
            return Ok(plugins);
        }

        let mut entries = tokio::fs::read_dir(&self.plugin_dir).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: format!("Failed to read plugin directory: {:?}", self.plugin_dir),
                details: e.to_string(),
            }
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: "Failed to read directory entry".to_string(),
                details: e.to_string(),
            }
        })? {
            let path = entry.path();
            if path.extension().map(|e| e == "wasm").unwrap_or(false) {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                plugins.push((name, path));
            }
        }

        Ok(plugins)
    }

    /// Load a plugin
    pub async fn load_plugin(
        &self,
        name: &str,
        config: Option<serde_json::Value>,
    ) -> crate::Result<Arc<PluginChannel>> {
        let wasm_path = self.plugin_dir.join(format!("{}.wasm", name));

        if !wasm_path.exists() {
            return Err(crate::error::MantaError::NotFound {
                resource: format!("Plugin '{}'", name),
            });
        }

        // Check if already loaded
        {
            let plugins = self.plugins.read().await;
            if let Some(plugin) = plugins.get(name) {
                return Ok(plugin.clone());
            }
        }

        // Load manifest if exists
        let manifest_path = self.plugin_dir.join(format!("{}.yaml", name));
        let config = if manifest_path.exists() {
            let manifest_yaml = tokio::fs::read_to_string(&manifest_path).await?;
            let _manifest: PluginManifest = serde_yaml::from_str(&manifest_yaml)?;
            config.unwrap_or_else(|| serde_json::json!({}))
        } else {
            config.unwrap_or_else(|| serde_json::json!({}))
        };

        // Load the plugin
        let plugin = PluginChannel::load(&wasm_path, config, self.message_tx.clone()).await?;

        // Initialize
        let _init_result = plugin.init(&serde_json::json!({})).await?;
        debug!("Plugin '{}' initialized", name);

        let plugin = Arc::new(plugin);

        // Store
        {
            let mut plugins = self.plugins.write().await;
            plugins.insert(name.to_string(), plugin.clone());
        }

        info!("Loaded plugin '{}'", name);
        Ok(plugin)
    }

    /// Unload a plugin
    pub async fn unload_plugin(&self, name: &str) -> crate::Result<()> {
        let mut plugins = self.plugins.write().await;

        if let Some(plugin) = plugins.remove(name) {
            let _ = plugin.stop().await;
            info!("Unloaded plugin '{}'", name);
        }

        Ok(())
    }

    /// Get a loaded plugin
    pub async fn get_plugin(&self, name: &str) -> Option<Arc<PluginChannel>> {
        let plugins = self.plugins.read().await;
        plugins.get(name).cloned()
    }

    /// List loaded plugins
    pub async fn list_loaded(&self) -> Vec<String> {
        let plugins = self.plugins.read().await;
        plugins.keys().cloned().collect()
    }

    /// Load all discovered plugins
    pub async fn load_all(&self) -> crate::Result<Vec<String>> {
        let discovered = self.discover_plugins().await?;
        let mut loaded = Vec::new();

        for (name, _) in discovered {
            match self.load_plugin(&name, None).await {
                Ok(_) => loaded.push(name),
                Err(e) => {
                    warn!("Failed to load plugin '{}': {}", name, e);
                }
            }
        }

        Ok(loaded)
    }

    /// Start all loaded plugins
    pub async fn start_all(&self) -> Vec<crate::Result<()>> {
        let plugins = self.plugins.read().await;
        let mut results = Vec::new();

        for (name, plugin) in plugins.iter() {
            let result = plugin.start().await;
            if let Err(ref e) = result {
                warn!("Failed to start plugin '{}': {}", name, e);
            }
            results.push(result);
        }

        results
    }

    /// Stop all loaded plugins
    pub async fn stop_all(&self) -> Vec<crate::Result<()>> {
        let plugins = self.plugins.read().await;
        let mut results = Vec::new();

        for (name, plugin) in plugins.iter() {
            let result = plugin.stop().await;
            if let Err(ref e) = result {
                warn!("Failed to stop plugin '{}': {}", name, e);
            }
            results.push(result);
        }

        results
    }
}

impl Default for PluginChannelRegistry {
    fn default() -> Self {
        let (message_tx, _) = mpsc::unbounded_channel();
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugin_dir: PathBuf::from("./plugins"),
            message_tx,
        }
    }
}
