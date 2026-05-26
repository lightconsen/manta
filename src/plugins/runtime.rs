//! Plugin Runtime - WASM-based plugin execution
//!
//! Loads and executes plugins using Wasmtime for sandboxing.

use super::manifest::PluginManifest;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// A loaded plugin instance
pub struct PluginInstance {
    /// Plugin manifest
    pub manifest: PluginManifest,
    /// Plugin directory path
    pub path: std::path::PathBuf,
    /// Whether the plugin is enabled
    pub enabled: bool,
    /// Plugin configuration
    pub config: serde_json::Value,
    /// WASM store (if loaded)
    #[cfg(feature = "plugins")]
    pub wasm_store: Option<wasmtime::Store<PluginState>>,
    #[cfg(feature = "plugins")]
    pub instance: Option<wasmtime::Instance>,
}

impl PluginInstance {
    /// Get plugin ID
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// Get plugin name
    pub fn name(&self) -> &str {
        &self.manifest.name
    }
}

/// Plugin state passed to WASM
#[cfg(feature = "plugins")]
pub struct PluginState {
    /// Plugin configuration
    pub config: serde_json::Value,
    /// Memory for plugin use
    pub memory: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

#[cfg(feature = "plugins")]
impl PluginState {
    pub fn new(config: serde_json::Value) -> Self {
        Self {
            config,
            memory: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn new_with_memory(config: serde_json::Value, memory: HashMap<String, Vec<u8>>) -> Self {
        Self {
            config,
            memory: Arc::new(RwLock::new(memory)),
        }
    }
}

/// Plugin runtime - manages plugin lifecycle
pub struct PluginRuntime {
    plugins: Arc<RwLock<HashMap<String, PluginInstance>>>,
    #[cfg(feature = "plugins")]
    engine: wasmtime::Engine,
    #[cfg(feature = "plugins")]
    linker: wasmtime::Linker<PluginState>,
}

impl PluginRuntime {
    /// Create a new plugin runtime
    pub fn new() -> crate::Result<Self> {
        #[cfg(feature = "plugins")]
        {
            let engine = wasmtime::Engine::default();
            let mut linker = wasmtime::Linker::new(&engine);

            // Define host functions for plugins
            Self::define_host_functions(&mut linker)?;

            Ok(Self {
                plugins: Arc::new(RwLock::new(HashMap::new())),
                engine,
                linker,
            })
        }

        #[cfg(not(feature = "plugins"))]
        {
            Ok(Self {
                plugins: Arc::new(RwLock::new(HashMap::new())),
            })
        }
    }

    #[cfg(feature = "plugins")]
    fn define_host_functions(linker: &mut wasmtime::Linker<PluginState>) -> crate::Result<()> {
        // Log function
        linker
            .func_wrap(
                "env",
                "log",
                |mut caller: wasmtime::Caller<'_, PluginState>, ptr: i32, len: i32| {
                    let memory = caller.get_export("memory").unwrap().into_memory().unwrap();
                    let data = memory.data(&caller);
                    let message = std::str::from_utf8(&data[ptr as usize..(ptr + len) as usize])
                        .unwrap_or("<invalid utf8>");
                    info!("[plugin] {}", message);
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // Config get function
        linker
            .func_wrap(
                "env",
                "config_get",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 key_ptr: i32,
                 key_len: i32,
                 out_ptr: i32,
                 out_len: i32|
                 -> i32 {
                    let memory = caller.get_export("memory").unwrap().into_memory().unwrap();
                    let data = memory.data(&caller);
                    let key =
                        std::str::from_utf8(&data[key_ptr as usize..(key_ptr + key_len) as usize])
                            .unwrap_or("");

                    let state = caller.data();
                    if let Some(value) = state.config.get(key) {
                        let value_str = value.to_string();
                        let bytes = value_str.as_bytes();
                        let to_write = bytes.len().min(out_len as usize);

                        let data_mut = memory.data_mut(&mut caller);
                        data_mut[out_ptr as usize..out_ptr as usize + to_write]
                            .copy_from_slice(&bytes[..to_write]);

                        to_write as i32
                    } else {
                        0
                    }
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        Ok(())
    }

    /// Load a plugin from a directory
    pub async fn load_plugin(&self, path: &std::path::Path) -> crate::Result<String> {
        let manifest_path = path.join("plugin.json");

        if !manifest_path.exists() {
            return Err(crate::error::ConfigError::Missing(format!(
                "Plugin manifest not found at {:?}",
                manifest_path
            ))
            .into());
        }

        let manifest_content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to read plugin manifest".to_string(),
                cause: Some(Box::new(e)),
            })?;

        let manifest: PluginManifest = serde_json::from_str(&manifest_content).map_err(|e| {
            crate::error::ConfigError::InvalidValue {
                key: "plugin.json".to_string(),
                message: format!("Invalid plugin manifest: {}", e),
            }
        })?;

        let plugin_id = manifest.id.clone();

        info!("Loading plugin '{}' ({}) from {:?}", manifest.name, plugin_id, path);

        // Load config if present
        let config_path = path.join("config.json");
        let config = if config_path.exists() {
            let config_content = tokio::fs::read_to_string(&config_path)
                .await
                .unwrap_or_default();
            serde_json::from_str(&config_content).unwrap_or(serde_json::json!({}))
        } else {
            manifest.config.clone().unwrap_or(serde_json::json!({}))
        };

        #[cfg(feature = "plugins")]
        let (wasm_store, instance) = {
            if let Some(ref main) = manifest.main {
                let wasm_path = path.join(main);
                if wasm_path.exists() {
                    self.load_wasm_plugin(&wasm_path, config.clone(), None)
                        .await?
                } else {
                    warn!("WASM file not found: {:?}", wasm_path);
                    (None, None)
                }
            } else {
                (None, None)
            }
        };

        let instance = PluginInstance {
            manifest,
            path: path.to_path_buf(),
            enabled: true,
            config,
            #[cfg(feature = "plugins")]
            wasm_store,
            #[cfg(feature = "plugins")]
            instance,
        };

        let mut plugins = self.plugins.write().await;
        plugins.insert(plugin_id.clone(), instance);

        info!("Plugin '{}' loaded successfully", plugin_id);

        Ok(plugin_id)
    }

    #[cfg(feature = "plugins")]
    async fn load_wasm_plugin(
        &self,
        wasm_path: &std::path::Path,
        config: serde_json::Value,
        preserved_memory: Option<HashMap<String, Vec<u8>>>,
    ) -> crate::Result<(Option<wasmtime::Store<PluginState>>, Option<wasmtime::Instance>)> {
        use wasmtime::Module;

        let wasm_bytes = tokio::fs::read(wasm_path).await.map_err(|e| {
            crate::error::MantaError::ExternalService {
                source: "Failed to read WASM file".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

        let module = Module::new(&self.engine, &wasm_bytes).map_err(|e| {
            crate::error::MantaError::Internal(format!("Failed to compile WASM: {}", e))
        })?;

        let state = if let Some(memory) = preserved_memory {
            PluginState::new_with_memory(config, memory)
        } else {
            PluginState::new(config)
        };
        let mut store = wasmtime::Store::new(&self.engine, state);

        let instance = self.linker.instantiate(&mut store, &module).map_err(|e| {
            crate::error::MantaError::Internal(format!("Failed to instantiate WASM: {}", e))
        })?;

        // Call init function if present
        if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "init") {
            init.call(&mut store, ()).map_err(|e| {
                crate::error::MantaError::Internal(format!("Plugin init failed: {}", e))
            })?;
        }

        Ok((Some(store), Some(instance)))
    }

    #[cfg(not(feature = "plugins"))]
    async fn load_wasm_plugin(
        &self,
        _wasm_path: &std::path::Path,
        _config: serde_json::Value,
    ) -> crate::Result<(Option<()>, Option<()>)> {
        Err(crate::error::MantaError::Internal(
            "Plugin execution requires the `plugins` feature. \
             Recompile Manta with `--features plugins` to enable WASM plugin support."
                .to_string(),
        ))
    }

    /// Unload a plugin
    pub async fn unload_plugin(&self, plugin_id: &str) -> crate::Result<bool> {
        let mut plugins = self.plugins.write().await;

        if let Some(plugin) = plugins.remove(plugin_id) {
            info!("Unloaded plugin '{}'", plugin.manifest.name);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Reload a plugin while preserving its runtime state (memory).
    ///
    /// Re-reads the manifest from disk so changes to `plugin.json` are picked up,
    /// then re-compiles and re-instantiates the WASM module, injecting the
    /// previously stored `PluginState::memory` into the new instance.
    pub async fn reload_plugin(&self, plugin_id: &str) -> crate::Result<String> {
        let mut plugins = self.plugins.write().await;
        let existing =
            plugins
                .get_mut(plugin_id)
                .ok_or_else(|| crate::error::ConfigError::InvalidValue {
                    key: "plugin_id".to_string(),
                    message: format!("Plugin '{}' not found", plugin_id),
                })?;

        let path = existing.path.clone();

        // Extract preserved memory before dropping the old store.
        let preserved_memory = if cfg!(feature = "plugins") {
            if let Some(store) = existing.wasm_store.take() {
                let state = store.into_data();
                let memory = state.memory.read().await.clone();
                Some(memory)
            } else {
                None
            }
        } else {
            None
        };

        drop(plugins);

        // Re-read manifest from disk to pick up edits.
        let manifest_path = path.join("plugin.json");
        let manifest_content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to read plugin manifest".to_string(),
                cause: Some(Box::new(e)),
            })?;

        let manifest: PluginManifest = serde_json::from_str(&manifest_content).map_err(|e| {
            crate::error::ConfigError::InvalidValue {
                key: "plugin.json".to_string(),
                message: format!("Invalid plugin manifest: {}", e),
            }
        })?;

        // Load config
        let config_path = path.join("config.json");
        let config = if config_path.exists() {
            let config_content = tokio::fs::read_to_string(&config_path)
                .await
                .unwrap_or_default();
            serde_json::from_str(&config_content).unwrap_or(serde_json::json!({}))
        } else {
            manifest.config.clone().unwrap_or(serde_json::json!({}))
        };

        // Re-compile WASM with preserved memory.
        #[cfg(feature = "plugins")]
        let (wasm_store, instance) = {
            if let Some(ref main) = manifest.main {
                let wasm_path = path.join(main);
                if wasm_path.exists() {
                    self.load_wasm_plugin(&wasm_path, config.clone(), preserved_memory)
                        .await?
                } else {
                    warn!("WASM file not found: {:?}", wasm_path);
                    (None, None)
                }
            } else {
                (None, None)
            }
        };

        #[cfg(not(feature = "plugins"))]
        let (wasm_store, instance) = (None, None);

        let new_instance = PluginInstance {
            manifest,
            path,
            enabled: true,
            config,
            #[cfg(feature = "plugins")]
            wasm_store,
            #[cfg(feature = "plugins")]
            instance,
        };

        let mut plugins = self.plugins.write().await;
        plugins.insert(plugin_id.to_string(), new_instance);

        info!("Plugin '{}' reloaded successfully", plugin_id);
        Ok(plugin_id.to_string())
    }

    /// Get a plugin instance
    pub async fn get_plugin(&self, plugin_id: &str) -> Option<PluginInstance> {
        let plugins = self.plugins.read().await;
        plugins.get(plugin_id).cloned()
    }

    /// List all loaded plugins
    pub async fn list_plugins(&self) -> Vec<PluginInstance> {
        let plugins = self.plugins.read().await;
        plugins.values().cloned().collect()
    }

    /// Enable/disable a plugin
    pub async fn set_enabled(&self, plugin_id: &str, enabled: bool) -> crate::Result<()> {
        let mut plugins = self.plugins.write().await;

        if let Some(plugin) = plugins.get_mut(plugin_id) {
            plugin.enabled = enabled;
            info!("Plugin '{}' {}", plugin_id, if enabled { "enabled" } else { "disabled" });
            Ok(())
        } else {
            Err(crate::error::ConfigError::InvalidValue {
                key: "plugin_id".to_string(),
                message: format!("Plugin '{}' not found", plugin_id),
            }
            .into())
        }
    }

    /// Call a tool provided by a plugin.
    ///
    /// The guest module is expected to export either:
    ///  - `call_tool(name_ptr: i32, name_len: i32, params_ptr: i32, params_len: i32,
    ///               out_ptr: i32, out_max: i32) -> i32`  (generic dispatcher), or
    ///  - `{tool_name}(params_ptr: i32, params_len: i32, out_ptr: i32, out_max: i32) -> i32`
    ///    (tool-specific function).
    ///
    /// The return value is the number of bytes written to `out_ptr`, or a negative
    /// value on error.  Both the input params and the output buffer are managed via
    /// the guest's `alloc(size: i32) -> i32` export when present.
    ///
    /// Params and results are JSON-encoded strings.
    pub async fn call_tool(
        &self,
        plugin_id: &str,
        tool_name: &str,
        params: serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        // We need a write lock so we can get `&mut Store` for WASM calls.
        let mut plugins = self.plugins.write().await;

        let plugin =
            plugins
                .get_mut(plugin_id)
                .ok_or_else(|| crate::error::ConfigError::InvalidValue {
                    key: "plugin_id".to_string(),
                    message: format!("Plugin '{}' not found", plugin_id),
                })?;

        if !plugin.enabled {
            return Err(crate::error::MantaError::Validation(format!(
                "Plugin '{}' is disabled",
                plugin_id
            )));
        }

        #[cfg(feature = "plugins")]
        {
            let (store, instance) = match (&mut plugin.wasm_store, &plugin.instance) {
                (Some(s), Some(i)) => (s, i),
                _ => {
                    return Err(crate::error::MantaError::Internal(format!(
                        "Plugin '{}' has no WASM module loaded",
                        plugin_id
                    )));
                }
            };

            Self::invoke_wasm_tool(store, instance, tool_name, params)
        }

        #[cfg(not(feature = "plugins"))]
        Err(crate::error::MantaError::Internal(
            "Plugin execution requires the `plugins` feature. \
             Recompile Manta with `--features plugins` to enable WASM plugin support."
                .to_string(),
        ))
    }

    /// Low-level WASM tool invocation.
    ///
    /// Writes the tool name and JSON-encoded params into guest memory (via the
    /// guest's `alloc` export), calls either the generic `call_tool` dispatcher
    /// or a per-tool export, then reads the JSON result back from guest memory.
    #[cfg(feature = "plugins")]
    fn invoke_wasm_tool(
        store: &mut wasmtime::Store<PluginState>,
        instance: &wasmtime::Instance,
        tool_name: &str,
        params: serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        const OUT_MAX: i32 = 65_536; // 64 KiB output buffer

        let params_json = serde_json::to_string(&params)
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;
        let tool_bytes = tool_name.as_bytes();
        let params_bytes = params_json.as_bytes();

        // Resolve the guest's linear memory.
        let memory = instance
            .get_export(&mut *store, "memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| {
                crate::error::MantaError::Internal(
                    "Plugin WASM module has no 'memory' export".to_string(),
                )
            })?;

        // Resolve the optional `alloc` export.  TypedFunc is Copy so we can
        // use it multiple times without re-borrowing.
        let alloc_fn: Option<wasmtime::TypedFunc<i32, i32>> = instance
            .get_typed_func::<i32, i32>(&mut *store, "alloc")
            .ok();

        // Allocate and write the tool name.
        let name_len = tool_bytes.len() as i32;
        let name_ptr = if let Some(ref f) = alloc_fn {
            f.call(&mut *store, name_len)
                .map_err(|e| crate::error::MantaError::Internal(format!("alloc: {}", e)))?
        } else {
            0i32
        };
        if name_ptr != 0 {
            let data = memory.data_mut(&mut *store);
            data[name_ptr as usize..name_ptr as usize + tool_bytes.len()]
                .copy_from_slice(tool_bytes);
        }

        // Allocate and write the JSON params.
        let params_len = params_bytes.len() as i32;
        let params_ptr = if let Some(ref f) = alloc_fn {
            f.call(&mut *store, params_len)
                .map_err(|e| crate::error::MantaError::Internal(format!("alloc: {}", e)))?
        } else {
            0i32
        };
        if params_ptr != 0 {
            let data = memory.data_mut(&mut *store);
            data[params_ptr as usize..params_ptr as usize + params_bytes.len()]
                .copy_from_slice(params_bytes);
        }

        // Allocate the output buffer.
        let out_ptr = if let Some(ref f) = alloc_fn {
            f.call(&mut *store, OUT_MAX)
                .map_err(|e| crate::error::MantaError::Internal(format!("alloc output: {}", e)))?
        } else {
            0i32
        };

        // Try the generic `call_tool` dispatcher first.
        let written: i32 = if let Ok(f) =
            instance.get_typed_func::<(i32, i32, i32, i32, i32, i32), i32>(&mut *store, "call_tool")
        {
            f.call(&mut *store, (name_ptr, name_len, params_ptr, params_len, out_ptr, OUT_MAX))
                .map_err(|e| crate::error::MantaError::Internal(format!("call_tool: {}", e)))?
        } else if let Ok(f) =
            instance.get_typed_func::<(i32, i32, i32, i32), i32>(&mut *store, tool_name)
        {
            // Fall back to a per-tool export.
            f.call(&mut *store, (params_ptr, params_len, out_ptr, OUT_MAX))
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("tool '{}': {}", tool_name, e))
                })?
        } else {
            return Err(crate::error::MantaError::Internal(format!(
                "Plugin does not export 'call_tool' or '{}' function",
                tool_name
            )));
        };

        if written < 0 {
            return Err(crate::error::MantaError::Internal(format!(
                "Plugin tool '{}' returned error code {}",
                tool_name, written
            )));
        }

        // Read the result JSON from the output buffer.
        let result_bytes = {
            let data = memory.data(&store);
            let start = out_ptr as usize;
            let end = start + written as usize;
            data[start..end].to_vec()
        };

        let result_str = std::str::from_utf8(&result_bytes).map_err(|e| {
            crate::error::MantaError::Internal(format!("Plugin returned invalid UTF-8: {}", e))
        })?;

        let result: serde_json::Value = serde_json::from_str(result_str)
            .unwrap_or_else(|_| serde_json::json!({ "output": result_str }));

        debug!("Plugin tool '{}' executed successfully ({} bytes)", tool_name, written);
        Ok(result)
    }

    // ------------------------------------------------------------------
    // Provider delegation stubs
    // ------------------------------------------------------------------

    /// Call a plugin's provider `complete` implementation.
    ///
    /// The plugin must export `provider_complete(request_ptr, request_len, out_ptr, out_max) -> i32`.
    pub async fn call_provider_complete(
        &self,
        plugin_id: &str,
        request: &serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        #[cfg(feature = "plugins")]
        {
            let mut plugins = self.plugins.write().await;
            let plugin = plugins.get_mut(plugin_id).ok_or_else(|| {
                crate::error::ConfigError::InvalidValue {
                    key: "plugin_id".to_string(),
                    message: format!("Plugin '{}' not found", plugin_id),
                }
            })?;

            if !plugin.enabled {
                return Err(crate::error::MantaError::Validation(format!(
                    "Plugin '{}' is disabled",
                    plugin_id
                )));
            }

            let (store, instance) = match (&mut plugin.wasm_store, &plugin.instance) {
                (Some(s), Some(i)) => (s, i),
                _ => {
                    return Err(crate::error::MantaError::Internal(format!(
                        "Plugin '{}' has no WASM module loaded",
                        plugin_id
                    )));
                }
            };

            Self::invoke_wasm_provider(store, instance, "provider_complete", request)
        }

        #[cfg(not(feature = "plugins"))]
        Err(crate::error::MantaError::Internal(
            "Plugin execution requires the `plugins` feature. \
             Recompile Manta with `--features plugins` to enable WASM plugin support."
                .to_string(),
        ))
    }

    /// Call a plugin's provider `stream` implementation.
    ///
    /// The plugin must export `provider_stream(request_ptr, request_len, out_ptr, out_max) -> i32`.
    /// Returns a JSON array of CompletionChunk objects.
    pub async fn call_provider_stream(
        &self,
        plugin_id: &str,
        request: &serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        #[cfg(feature = "plugins")]
        {
            let mut plugins = self.plugins.write().await;
            let plugin = plugins.get_mut(plugin_id).ok_or_else(|| {
                crate::error::ConfigError::InvalidValue {
                    key: "plugin_id".to_string(),
                    message: format!("Plugin '{}' not found", plugin_id),
                }
            })?;

            if !plugin.enabled {
                return Err(crate::error::MantaError::Validation(format!(
                    "Plugin '{}' is disabled",
                    plugin_id
                )));
            }

            let (store, instance) = match (&mut plugin.wasm_store, &plugin.instance) {
                (Some(s), Some(i)) => (s, i),
                _ => {
                    return Err(crate::error::MantaError::Internal(format!(
                        "Plugin '{}' has no WASM module loaded",
                        plugin_id
                    )));
                }
            };

            Self::invoke_wasm_provider(store, instance, "provider_stream", request)
        }

        #[cfg(not(feature = "plugins"))]
        Err(crate::error::MantaError::Internal(
            "Plugin execution requires the `plugins` feature. \
             Recompile Manta with `--features plugins` to enable WASM plugin support."
                .to_string(),
        ))
    }

    /// Call a plugin's provider `health_check` implementation.
    ///
    /// The plugin must export `provider_health_check(out_ptr, out_max) -> i32`.
    pub async fn call_provider_health_check(
        &self,
        plugin_id: &str,
    ) -> crate::Result<serde_json::Value> {
        #[cfg(feature = "plugins")]
        {
            let mut plugins = self.plugins.write().await;
            let plugin = plugins.get_mut(plugin_id).ok_or_else(|| {
                crate::error::ConfigError::InvalidValue {
                    key: "plugin_id".to_string(),
                    message: format!("Plugin '{}' not found", plugin_id),
                }
            })?;

            if !plugin.enabled {
                return Err(crate::error::MantaError::Validation(format!(
                    "Plugin '{}' is disabled",
                    plugin_id
                )));
            }

            let (store, instance) = match (&mut plugin.wasm_store, &plugin.instance) {
                (Some(s), Some(i)) => (s, i),
                _ => {
                    return Err(crate::error::MantaError::Internal(format!(
                        "Plugin '{}' has no WASM module loaded",
                        plugin_id
                    )));
                }
            };

            Self::invoke_wasm_provider(
                store,
                instance,
                "provider_health_check",
                &serde_json::json!({}),
            )
        }

        #[cfg(not(feature = "plugins"))]
        Err(crate::error::MantaError::Internal(
            "Plugin execution requires the `plugins` feature. \
             Recompile Manta with `--features plugins` to enable WASM plugin support."
                .to_string(),
        ))
    }

    /// Low-level WASM provider invocation.
    #[cfg(feature = "plugins")]
    fn invoke_wasm_provider(
        store: &mut wasmtime::Store<PluginState>,
        instance: &wasmtime::Instance,
        export_name: &str,
        request: &serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        const OUT_MAX: i32 = 256_000; // 256 KiB output buffer

        let request_json = serde_json::to_string(request)
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;
        let request_bytes = request_json.as_bytes();

        let memory = instance
            .get_export(&mut *store, "memory")
            .and_then(|e| e.into_memory())
            .ok_or_else(|| {
                crate::error::MantaError::Internal(
                    "Plugin WASM module has no 'memory' export".to_string(),
                )
            })?;

        let alloc_fn: Option<wasmtime::TypedFunc<i32, i32>> = instance
            .get_typed_func::<i32, i32>(&mut *store, "alloc")
            .ok();

        let req_len = request_bytes.len() as i32;
        let req_ptr = if let Some(ref f) = alloc_fn {
            f.call(&mut *store, req_len)
                .map_err(|e| crate::error::MantaError::Internal(format!("alloc: {}", e)))?
        } else {
            0i32
        };
        if req_ptr != 0 {
            let data = memory.data_mut(&mut *store);
            data[req_ptr as usize..req_ptr as usize + request_bytes.len()]
                .copy_from_slice(request_bytes);
        }

        let out_ptr = if let Some(ref f) = alloc_fn {
            f.call(&mut *store, OUT_MAX)
                .map_err(|e| crate::error::MantaError::Internal(format!("alloc output: {}", e)))?
        } else {
            0i32
        };

        let written: i32 = if let Ok(f) =
            instance.get_typed_func::<(i32, i32, i32, i32), i32>(&mut *store, export_name)
        {
            f.call(&mut *store, (req_ptr, req_len, out_ptr, OUT_MAX))
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("{}: {}", export_name, e))
                })?
        } else {
            return Err(crate::error::MantaError::Internal(format!(
                "Plugin does not export '{}' function",
                export_name
            )));
        };

        if written < 0 {
            return Err(crate::error::MantaError::Internal(format!(
                "Plugin provider '{}' returned error code {}",
                export_name, written
            )));
        }

        let result_bytes = {
            let data = memory.data(&store);
            let start = out_ptr as usize;
            let end = start + written as usize;
            data[start..end].to_vec()
        };

        let result_str = std::str::from_utf8(&result_bytes).map_err(|e| {
            crate::error::MantaError::Internal(format!("Plugin returned invalid UTF-8: {}", e))
        })?;

        let result: serde_json::Value = serde_json::from_str(result_str)
            .unwrap_or_else(|_| serde_json::json!({ "output": result_str }));

        debug!("Plugin provider '{}' executed successfully ({} bytes)", export_name, written);
        Ok(result)
    }

    /// Shutdown all plugins
    pub async fn shutdown(&self) -> crate::Result<()> {
        let mut plugins = self.plugins.write().await;

        for (id, _plugin) in plugins.drain() {
            info!("Shutting down plugin '{}'", id);
        }

        Ok(())
    }
}

impl Default for PluginRuntime {
    fn default() -> Self {
        Self::new().expect("Failed to create plugin runtime")
    }
}

impl Clone for PluginInstance {
    fn clone(&self) -> Self {
        // Note: WASM stores can't be cloned, so we skip them
        Self {
            manifest: self.manifest.clone(),
            path: self.path.clone(),
            enabled: self.enabled,
            config: self.config.clone(),
            #[cfg(feature = "plugins")]
            wasm_store: None,
            #[cfg(feature = "plugins")]
            instance: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: id.to_string(),
            name: format!("Test Plugin {}", id),
            version: "1.0.0".to_string(),
            description: "A test plugin".to_string(),
            author: None,
            main: None,
            capabilities: None,
            permissions: None,
            config: None,
        }
    }

    fn test_instance(id: &str) -> PluginInstance {
        PluginInstance {
            manifest: test_manifest(id),
            path: std::path::PathBuf::from("/tmp/test-plugin"),
            enabled: true,
            config: serde_json::json!({}),
            #[cfg(feature = "plugins")]
            wasm_store: None,
            #[cfg(feature = "plugins")]
            instance: None,
        }
    }

    #[test]
    fn test_plugin_runtime_new() {
        let runtime = PluginRuntime::new();
        assert!(runtime.is_ok());
    }

    #[test]
    fn test_plugin_instance_getters() {
        let instance = test_instance("com.test.plugin");
        assert_eq!(instance.id(), "com.test.plugin");
        assert_eq!(instance.name(), "Test Plugin com.test.plugin");
    }

    #[test]
    fn test_plugin_instance_clone_drops_wasm() {
        let instance = test_instance("com.test.plugin");
        let cloned = instance.clone();
        assert_eq!(cloned.id(), "com.test.plugin");
        assert_eq!(cloned.enabled, true);
        #[cfg(feature = "plugins")]
        {
            assert!(cloned.wasm_store.is_none());
            assert!(cloned.instance.is_none());
        }
    }

    #[tokio::test]
    async fn test_load_plugin_from_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest = serde_json::json!({
            "id": "com.test.loader",
            "name": "Loader Test",
            "version": "0.1.0",
            "description": "Testing load_plugin"
        });
        tokio::fs::write(
            temp_dir.path().join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let runtime = PluginRuntime::new().unwrap();
        let plugin_id = runtime.load_plugin(temp_dir.path()).await.unwrap();
        assert_eq!(plugin_id, "com.test.loader");

        let plugin = runtime.get_plugin("com.test.loader").await.unwrap();
        assert_eq!(plugin.name(), "Loader Test");
    }

    #[tokio::test]
    async fn test_load_plugin_missing_manifest() {
        let temp_dir = tempfile::tempdir().unwrap();
        let runtime = PluginRuntime::new().unwrap();

        let result = runtime.load_plugin(temp_dir.path()).await;
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("Plugin manifest not found"));
    }

    #[tokio::test]
    async fn test_get_plugin_not_found() {
        let runtime = PluginRuntime::new().unwrap();
        let plugin = runtime.get_plugin("nonexistent").await;
        assert!(plugin.is_none());
    }

    #[tokio::test]
    async fn test_list_plugins_empty() {
        let runtime = PluginRuntime::new().unwrap();
        let plugins = runtime.list_plugins().await;
        assert!(plugins.is_empty());
    }

    #[tokio::test]
    async fn test_list_plugins_after_load() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest = serde_json::json!({
            "id": "com.test.list",
            "name": "List Test",
            "version": "0.1.0",
            "description": "Testing list_plugins"
        });
        tokio::fs::write(
            temp_dir.path().join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let runtime = PluginRuntime::new().unwrap();
        runtime.load_plugin(temp_dir.path()).await.unwrap();

        let plugins = runtime.list_plugins().await;
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id(), "com.test.list");
    }

    #[tokio::test]
    async fn test_set_enabled() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest = serde_json::json!({
            "id": "com.test.toggle",
            "name": "Toggle Test",
            "version": "0.1.0",
            "description": "Testing set_enabled"
        });
        tokio::fs::write(
            temp_dir.path().join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let runtime = PluginRuntime::new().unwrap();
        runtime.load_plugin(temp_dir.path()).await.unwrap();

        // Disable
        runtime.set_enabled("com.test.toggle", false).await.unwrap();
        let plugin = runtime.get_plugin("com.test.toggle").await.unwrap();
        assert!(!plugin.enabled);

        // Enable
        runtime.set_enabled("com.test.toggle", true).await.unwrap();
        let plugin = runtime.get_plugin("com.test.toggle").await.unwrap();
        assert!(plugin.enabled);
    }

    #[tokio::test]
    async fn test_set_enabled_not_found() {
        let runtime = PluginRuntime::new().unwrap();
        let result = runtime.set_enabled("nonexistent", false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unload_plugin() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest = serde_json::json!({
            "id": "com.test.unload",
            "name": "Unload Test",
            "version": "0.1.0",
            "description": "Testing unload_plugin"
        });
        tokio::fs::write(
            temp_dir.path().join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let runtime = PluginRuntime::new().unwrap();
        runtime.load_plugin(temp_dir.path()).await.unwrap();
        assert!(runtime.get_plugin("com.test.unload").await.is_some());

        let removed = runtime.unload_plugin("com.test.unload").await.unwrap();
        assert!(removed);
        assert!(runtime.get_plugin("com.test.unload").await.is_none());
    }

    #[tokio::test]
    async fn test_unload_plugin_not_found() {
        let runtime = PluginRuntime::new().unwrap();
        let removed = runtime.unload_plugin("nonexistent").await.unwrap();
        assert!(!removed);
    }

    #[tokio::test]
    async fn test_shutdown() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manifest = serde_json::json!({
            "id": "com.test.shutdown",
            "name": "Shutdown Test",
            "version": "0.1.0",
            "description": "Testing shutdown"
        });
        tokio::fs::write(
            temp_dir.path().join("plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .await
        .unwrap();

        let runtime = PluginRuntime::new().unwrap();
        runtime.load_plugin(temp_dir.path()).await.unwrap();

        let result = runtime.shutdown().await;
        assert!(result.is_ok());

        // After shutdown, plugin list should be empty
        let plugins = runtime.list_plugins().await;
        assert!(plugins.is_empty());
    }

    #[test]
    fn test_plugin_state_new() {
        let state = PluginState::new(serde_json::json!({"key": "value"}));
        assert_eq!(state.config["key"], "value");
    }

    #[test]
    fn test_plugin_state_new_with_memory() {
        let mut memory = HashMap::new();
        memory.insert("data".to_string(), vec![1, 2, 3]);
        let state = PluginState::new_with_memory(serde_json::json!({}), memory);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let stored = rt.block_on(async {
            let m = state.memory.read().await;
            m.get("data").cloned()
        });
        assert_eq!(stored, Some(vec![1, 2, 3]));
    }
}
