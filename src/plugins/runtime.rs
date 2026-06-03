//! Plugin Runtime - WASM-based plugin execution
//!
//! Loads and executes plugins using Wasmtime for sandboxing.

use super::manifest::PluginManifest;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Shared state accessible by all plugin instances.
///
/// Provides async-capable primitives (KV store, HTTP client, event bridge)
/// that synchronous WASM host functions delegate to via `block_on`.
#[cfg(feature = "plugins")]
#[derive(Default)]
pub struct PluginSharedState {
    /// Persistent per-plugin KV store
    pub kv_store: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    /// Global event channel (plugins emit, Manta consumers subscribe)
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<PluginEvent>>,
    /// Shared HTTP client
    pub http_client: reqwest::Client,
    /// Current session ID (set by Manta when invoking plugins)
    pub session_id: Arc<RwLock<Option<String>>>,
    /// Arbitrary context map (set by Manta)
    pub context: Arc<RwLock<HashMap<String, String>>>,
}

#[cfg(feature = "plugins")]
impl PluginSharedState {
    /// Create shared state without event channel
    pub fn new() -> Self {
        Self {
            kv_store: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            http_client: reqwest::Client::new(),
            session_id: Arc::new(RwLock::new(None)),
            context: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create shared state with an event channel
    pub fn with_events(event_tx: tokio::sync::mpsc::UnboundedSender<PluginEvent>) -> Self {
        Self {
            kv_store: Arc::new(RwLock::new(HashMap::new())),
            event_tx: Some(event_tx),
            http_client: reqwest::Client::new(),
            session_id: Arc::new(RwLock::new(None)),
            context: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set the current session ID
    pub async fn set_session_id(&self, id: String) {
        *self.session_id.write().await = Some(id);
    }

    /// Get the current session ID
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Set a context value
    pub async fn set_context(&self, key: String, value: String) {
        self.context.write().await.insert(key, value);
    }

    /// Get a context value
    pub async fn get_context(&self, key: &str) -> Option<String> {
        self.context.read().await.get(key).cloned()
    }

    /// Get all context as JSON
    pub async fn get_all_context(&self) -> String {
        let ctx = self.context.read().await;
        serde_json::to_string(&*ctx).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Event emitted by plugins via `emit_event`
#[cfg(feature = "plugins")]
#[derive(Debug, Clone)]
pub struct PluginEvent {
    pub plugin_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
}

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
    /// Shared state (KV store, HTTP, events, context)
    pub shared_state: Arc<PluginSharedState>,
    /// Plugin ID (for event emission, store scoping)
    pub plugin_id: String,
}

#[cfg(feature = "plugins")]
impl PluginState {
    pub fn new(
        config: serde_json::Value,
        shared_state: Arc<PluginSharedState>,
        plugin_id: String,
    ) -> Self {
        Self {
            config,
            memory: Arc::new(RwLock::new(HashMap::new())),
            shared_state,
            plugin_id,
        }
    }

    pub fn new_with_memory(
        config: serde_json::Value,
        memory: HashMap<String, Vec<u8>>,
        shared_state: Arc<PluginSharedState>,
        plugin_id: String,
    ) -> Self {
        Self {
            config,
            memory: Arc::new(RwLock::new(memory)),
            shared_state,
            plugin_id,
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
    #[cfg(feature = "plugins")]
    shared_state: Arc<PluginSharedState>,
}

impl PluginRuntime {
    /// Create a new plugin runtime
    pub fn new() -> crate::Result<Self> {
        #[cfg(feature = "plugins")]
        {
            let engine = wasmtime::Engine::default();
            let mut linker = wasmtime::Linker::new(&engine);
            let shared_state = Arc::new(PluginSharedState::new());

            // Define host functions for plugins
            Self::define_host_functions(&mut linker)?;

            Ok(Self {
                plugins: Arc::new(RwLock::new(HashMap::new())),
                engine,
                linker,
                shared_state,
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
        use wasmtime::Memory;

        // Helper: read a UTF-8 string from WASM memory
        fn read_memory_string(
            memory: &Memory,
            caller: &mut wasmtime::Caller<'_, PluginState>,
            ptr: i32,
            len: i32,
        ) -> anyhow::Result<String> {
            let data = memory.data(caller);
            let bytes = &data[ptr as usize..(ptr + len) as usize];
            Ok(std::str::from_utf8(bytes)
                .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in WASM memory: {}", e))?
                .to_string())
        }

        // --- Logging ---

        // log(ptr, len)
        linker
            .func_wrap(
                "env",
                "log",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 ptr: i32,
                 len: i32|
                 -> anyhow::Result<()> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let data = memory.data(&caller);
                    let message = std::str::from_utf8(&data[ptr as usize..(ptr + len) as usize])
                        .unwrap_or("<invalid utf8>");
                    info!("[plugin] {}", message);
                    Ok(())
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // --- Config ---

        // config_get(key_ptr, key_len, out_ptr, out_len) -> bytes_written | 0
        linker
            .func_wrap(
                "env",
                "config_get",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 key_ptr: i32,
                 key_len: i32,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let key = read_memory_string(&memory, &mut caller, key_ptr, key_len)?;
                    let state = caller.data();
                    if let Some(value) = state.config.get(&key) {
                        let value_str = value.to_string();
                        let bytes = value_str.as_bytes();
                        let to_write = bytes.len().min(out_len as usize);
                        let data_mut = memory.data_mut(&mut caller);
                        data_mut[out_ptr as usize..out_ptr as usize + to_write]
                            .copy_from_slice(&bytes[..to_write]);
                        Ok(to_write as i32)
                    } else {
                        Ok(0)
                    }
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // config_get_all(out_ptr, out_len) -> bytes_written
        linker
            .func_wrap(
                "env",
                "config_get_all",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let state = caller.data();
                    let config_str =
                        serde_json::to_string(&state.config).unwrap_or_else(|_| "{}".to_string());
                    let bytes = config_str.as_bytes();
                    let to_write = bytes.len().min(out_len as usize);
                    let data_mut = memory.data_mut(&mut caller);
                    data_mut[out_ptr as usize..out_ptr as usize + to_write]
                        .copy_from_slice(&bytes[..to_write]);
                    Ok(to_write as i32)
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // --- In-memory Store (per-plugin HashMap) ---

        // memory_store(key_ptr, key_len, val_ptr, val_len) -> 1 on success
        linker
            .func_wrap(
                "env",
                "memory_store",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 key_ptr: i32,
                 key_len: i32,
                 val_ptr: i32,
                 val_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let key = read_memory_string(&memory, &mut caller, key_ptr, key_len)?;
                    let value: Vec<u8> = memory.data(&caller)
                        [val_ptr as usize..(val_ptr + val_len) as usize]
                        .to_vec();
                    let state = caller.data();
                    let rt = tokio::runtime::Handle::current();
                    let mem = state.memory.clone();
                    Ok(rt.block_on(async move {
                        mem.write().await.insert(key, value);
                        1i32
                    }))
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // memory_load(key_ptr, key_len, out_ptr, out_len) -> bytes_written | 0
        linker
            .func_wrap(
                "env",
                "memory_load",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 key_ptr: i32,
                 key_len: i32,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let key = read_memory_string(&memory, &mut caller, key_ptr, key_len)?;
                    let state = caller.data();
                    let rt = tokio::runtime::Handle::current();
                    let mem = state.memory.clone();
                    let out_ptr = out_ptr as usize;
                    let out_len = out_len as usize;
                    Ok(rt.block_on(async move {
                        if let Some(data) = mem.read().await.get(&key) {
                            let to_write = data.len().min(out_len);
                            let mem_data = memory.data_mut(&mut caller);
                            mem_data[out_ptr..out_ptr + to_write]
                                .copy_from_slice(&data[..to_write]);
                            to_write as i32
                        } else {
                            0i32
                        }
                    }))
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // memory_search(prefix_ptr, prefix_len, out_ptr, out_len) -> bytes_written
        linker
            .func_wrap(
                "env",
                "memory_search",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 prefix_ptr: i32,
                 prefix_len: i32,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let prefix = read_memory_string(&memory, &mut caller, prefix_ptr, prefix_len)?;
                    let state = caller.data();
                    let rt = tokio::runtime::Handle::current();
                    let mem = state.memory.clone();
                    let out_ptr = out_ptr as usize;
                    let out_len = out_len as usize;
                    Ok(rt.block_on(async move {
                        let mem_guard = mem.read().await;
                        let keys: Vec<String> = mem_guard
                            .keys()
                            .filter(|k| k.starts_with(&prefix))
                            .cloned()
                            .collect();
                        drop(mem_guard);
                        let result =
                            serde_json::to_string(&keys).unwrap_or_else(|_| "[]".to_string());
                        let bytes = result.as_bytes();
                        let to_write = bytes.len().min(out_len);
                        let mem_data = memory.data_mut(&mut caller);
                        mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&bytes[..to_write]);
                        to_write as i32
                    }))
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // --- Persistent KV Store (global, scoped by plugin_id) ---

        // store_get(key_ptr, key_len, out_ptr, out_len) -> bytes_written | 0
        linker
            .func_wrap(
                "env",
                "store_get",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 key_ptr: i32,
                 key_len: i32,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let key = read_memory_string(&memory, &mut caller, key_ptr, key_len)?;
                    let state = caller.data();
                    let plugin_id = state.plugin_id.clone();
                    let kv = state.shared_state.kv_store.clone();
                    let rt = tokio::runtime::Handle::current();
                    let out_ptr = out_ptr as usize;
                    let out_len = out_len as usize;
                    Ok(rt.block_on(async move {
                        let store = kv.read().await;
                        let value = store.get(&plugin_id).and_then(|m| m.get(&key)).cloned();
                        drop(store);
                        if let Some(value) = value {
                            let bytes = value.as_bytes();
                            let to_write = bytes.len().min(out_len);
                            let mem_data = memory.data_mut(&mut caller);
                            mem_data[out_ptr..out_ptr + to_write]
                                .copy_from_slice(&bytes[..to_write]);
                            to_write as i32
                        } else {
                            0i32
                        }
                    }))
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // store_set(key_ptr, key_len, val_ptr, val_len) -> 1 on success
        linker
            .func_wrap(
                "env",
                "store_set",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 key_ptr: i32,
                 key_len: i32,
                 val_ptr: i32,
                 val_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let key = read_memory_string(&memory, &mut caller, key_ptr, key_len)?;
                    let value = read_memory_string(&memory, &mut caller, val_ptr, val_len)?;
                    let state = caller.data();
                    let plugin_id = state.plugin_id.clone();
                    let kv = state.shared_state.kv_store.clone();
                    let rt = tokio::runtime::Handle::current();
                    Ok(rt.block_on(async move {
                        kv.write()
                            .await
                            .entry(plugin_id)
                            .or_default()
                            .insert(key, value);
                        1i32
                    }))
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // --- HTTP ---

        // http_get(url_ptr, url_len, out_ptr, out_len) -> bytes_written
        linker
            .func_wrap(
                "env",
                "http_get",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 url_ptr: i32,
                 url_len: i32,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let url = read_memory_string(&memory, &mut caller, url_ptr, url_len)?;
                    let out_ptr = out_ptr as usize;
                    let out_len = out_len as usize;
                    let body = reqwest::blocking::get(&url)
                        .and_then(|r| r.text())
                        .unwrap_or_else(|e| format!("{{\"error\":\"{}\"}}", e));
                    let bytes = body.as_bytes();
                    let to_write = bytes.len().min(out_len);
                    let mem_data = memory.data_mut(&mut caller);
                    mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&bytes[..to_write]);
                    Ok(to_write as i32)
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // http_post(url_ptr, url_len, body_ptr, body_len, content_type_ptr, content_type_len, out_ptr, out_len) -> bytes_written
        linker
            .func_wrap(
                "env",
                "http_post",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 url_ptr: i32,
                 url_len: i32,
                 body_ptr: i32,
                 body_len: i32,
                 ct_ptr: i32,
                 ct_len: i32,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let url = read_memory_string(&memory, &mut caller, url_ptr, url_len)?;
                    let body = read_memory_string(&memory, &mut caller, body_ptr, body_len)?;
                    let content_type = read_memory_string(&memory, &mut caller, ct_ptr, ct_len)?;
                    let out_ptr = out_ptr as usize;
                    let out_len = out_len as usize;
                    let response = reqwest::blocking::Client::new()
                        .post(&url)
                        .header("Content-Type", &content_type)
                        .body(body)
                        .send()
                        .and_then(|r| r.text())
                        .unwrap_or_else(|e| format!("{{\"error\":\"{}\"}}", e));
                    let bytes = response.as_bytes();
                    let to_write = bytes.len().min(out_len);
                    let mem_data = memory.data_mut(&mut caller);
                    mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&bytes[..to_write]);
                    Ok(to_write as i32)
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // --- Events ---

        // emit_event(type_ptr, type_len, payload_ptr, payload_len) -> 1 | 0 (no channel)
        linker
            .func_wrap(
                "env",
                "emit_event",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 type_ptr: i32,
                 type_len: i32,
                 payload_ptr: i32,
                 payload_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let event_type = read_memory_string(&memory, &mut caller, type_ptr, type_len)?;
                    let payload_str =
                        read_memory_string(&memory, &mut caller, payload_ptr, payload_len)?;
                    let payload: serde_json::Value = serde_json::from_str(&payload_str)
                        .unwrap_or_else(|_| serde_json::json!({ "raw": payload_str }));
                    let state = caller.data();
                    let plugin_id = state.plugin_id.clone();
                    if let Some(ref tx) = state.shared_state.event_tx {
                        let _ = tx.send(PluginEvent { plugin_id, event_type, payload });
                        Ok(1)
                    } else {
                        Ok(0)
                    }
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // --- Context ---

        // get_context(key_ptr, key_len, out_ptr, out_len) -> bytes_written | 0
        linker
            .func_wrap(
                "env",
                "get_context",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 key_ptr: i32,
                 key_len: i32,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let key = read_memory_string(&memory, &mut caller, key_ptr, key_len)?;
                    let state = caller.data();
                    let ctx = state.shared_state.context.clone();
                    let rt = tokio::runtime::Handle::current();
                    let out_ptr = out_ptr as usize;
                    let out_len = out_len as usize;
                    Ok(rt.block_on(async move {
                        if let Some(value) = ctx.read().await.get(&key).cloned() {
                            let bytes = value.as_bytes();
                            let to_write = bytes.len().min(out_len);
                            let mem_data = memory.data_mut(&mut caller);
                            mem_data[out_ptr..out_ptr + to_write]
                                .copy_from_slice(&bytes[..to_write]);
                            to_write as i32
                        } else {
                            0i32
                        }
                    }))
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // get_session_id(out_ptr, out_len) -> bytes_written | 0
        linker
            .func_wrap(
                "env",
                "get_session_id",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let state = caller.data();
                    let sid = state.shared_state.session_id.clone();
                    let rt = tokio::runtime::Handle::current();
                    let out_ptr = out_ptr as usize;
                    let out_len = out_len as usize;
                    Ok(rt.block_on(async move {
                        if let Some(session_id) = sid.read().await.clone() {
                            let bytes = session_id.as_bytes();
                            let to_write = bytes.len().min(out_len);
                            let mem_data = memory.data_mut(&mut caller);
                            mem_data[out_ptr..out_ptr + to_write]
                                .copy_from_slice(&bytes[..to_write]);
                            to_write as i32
                        } else {
                            0i32
                        }
                    }))
                },
            )
            .map_err(|e| crate::error::MantaError::Internal(e.to_string()))?;

        // --- Plugin Info ---

        // get_plugin_id(out_ptr, out_len) -> bytes_written
        linker
            .func_wrap(
                "env",
                "get_plugin_id",
                |mut caller: wasmtime::Caller<'_, PluginState>,
                 out_ptr: i32,
                 out_len: i32|
                 -> anyhow::Result<i32> {
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            anyhow::anyhow!("Plugin does not export a memory segment")
                        })?;
                    let state = caller.data();
                    let plugin_id = state.plugin_id.clone();
                    let bytes = plugin_id.as_bytes();
                    let to_write = bytes.len().min(out_len as usize);
                    let data_mut = memory.data_mut(&mut caller);
                    data_mut[out_ptr as usize..out_ptr as usize + to_write]
                        .copy_from_slice(&bytes[..to_write]);
                    Ok(to_write as i32)
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
                    self.load_wasm_plugin(&wasm_path, config.clone(), &plugin_id, None)
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
        plugin_id: &str,
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
            PluginState::new_with_memory(
                config,
                memory,
                self.shared_state.clone(),
                plugin_id.to_string(),
            )
        } else {
            PluginState::new(config, self.shared_state.clone(), plugin_id.to_string())
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
                    self.load_wasm_plugin(&wasm_path, config.clone(), plugin_id, preserved_memory)
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
        let shared_state = Arc::new(PluginSharedState::new());
        let state = PluginState::new(
            serde_json::json!({"key": "value"}),
            shared_state,
            "test.plugin".to_string(),
        );
        assert_eq!(state.config["key"], "value");
    }

    #[test]
    fn test_plugin_state_new_with_memory() {
        let shared_state = Arc::new(PluginSharedState::new());
        let mut memory = HashMap::new();
        memory.insert("data".to_string(), vec![1, 2, 3]);
        let state = PluginState::new_with_memory(
            serde_json::json!({}),
            memory,
            shared_state,
            "test.plugin".to_string(),
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        let stored = rt.block_on(async {
            let m = state.memory.read().await;
            m.get("data").cloned()
        });
        assert_eq!(stored, Some(vec![1, 2, 3]));
    }
}
