//! Plugin Runtime - WASM-based plugin execution
//!
//! Loads and executes plugins using Wasmtime for sandboxing.

use super::manifest::{PluginManifest, PluginTool};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

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
                    self.load_wasm_plugin(&wasm_path, config.clone()).await?
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

        let state = PluginState::new(config);
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
        Ok((None, None))
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
            let (store, instance) =
                match (&mut plugin.wasm_store, &plugin.instance) {
                    (Some(s), Some(i)) => (s, i),
                    _ => {
                        return Err(crate::error::MantaError::Internal(format!(
                            "Plugin '{}' has no WASM module loaded",
                            plugin_id
                        )));
                    }
                };

            return Self::invoke_wasm_tool(store, instance, tool_name, params);
        }

        #[cfg(not(feature = "plugins"))]
        Err(crate::error::MantaError::Internal(
            "plugins feature is not enabled".to_string(),
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
        let alloc_fn: Option<wasmtime::TypedFunc<i32, i32>> =
            instance.get_typed_func::<i32, i32>(&mut *store, "alloc").ok();

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
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("alloc output: {}", e))
                })?
        } else {
            0i32
        };

        // Try the generic `call_tool` dispatcher first.
        let written: i32 = if let Ok(f) = instance.get_typed_func::<(i32, i32, i32, i32, i32, i32), i32>(
            &mut *store,
            "call_tool",
        ) {
            f.call(
                &mut *store,
                (name_ptr, name_len, params_ptr, params_len, out_ptr, OUT_MAX),
            )
            .map_err(|e| {
                crate::error::MantaError::Internal(format!("call_tool: {}", e))
            })?
        } else if let Ok(f) =
            instance.get_typed_func::<(i32, i32, i32, i32), i32>(&mut *store, tool_name)
        {
            // Fall back to a per-tool export.
            f.call(&mut *store, (params_ptr, params_len, out_ptr, OUT_MAX))
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!(
                        "tool '{}': {}",
                        tool_name, e
                    ))
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
            crate::error::MantaError::Internal(format!(
                "Plugin returned invalid UTF-8: {}",
                e
            ))
        })?;

        let result: serde_json::Value =
            serde_json::from_str(result_str).unwrap_or_else(|_| {
                serde_json::json!({ "output": result_str })
            });

        debug!(
            "Plugin tool '{}' executed successfully ({} bytes)",
            tool_name, written
        );
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
