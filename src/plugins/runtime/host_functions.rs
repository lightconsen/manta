//! WASM host function definitions for the plugin runtime.
//!
//! Note: This entire module is `#[cfg(feature = "plugins")]`-gated from
//! `runtime/mod.rs`, so no per-item cfg annotations are needed.

use tracing::info;

use super::super::manifest::PluginPermission;
use super::state::{PluginEvent, PluginState};

/// Define all host functions available to WASM plugins.
///
/// Registers 16 host functions (logging, config, memory, KV store, HTTP,
/// events, context, plugin info) plus WASI support on the given linker.
pub(crate) fn define_host_functions(
    linker: &mut wasmtime::Linker<PluginState>,
) -> crate::Result<()> {
    use wasmtime::Memory;

    // Register WASI in the linker so plugins can use WASI APIs
    wasmtime_wasi::p1::add_to_linker_sync(linker, |state: &mut PluginState| {
        #[allow(clippy::expect_used)] // wasi_ctx always set during plugin construction
        state.wasi_ctx.get_mut().expect("wasi context initialized")
    })
    .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

    // Helper: read a UTF-8 string from WASM memory
    fn read_memory_string(
        memory: &Memory,
        caller: &mut wasmtime::Caller<'_, PluginState>,
        ptr: i32,
        len: i32,
    ) -> wasmtime::Result<String> {
        let data = memory.data(caller);
        let bytes = &data[ptr as usize..(ptr + len) as usize];
        Ok(std::str::from_utf8(bytes)
            .map_err(|e| wasmtime::Error::msg(format!("Invalid UTF-8 in WASM memory: {}", e)))?
            .to_string())
    }

    // Permission-check helpers
    fn check_store_permission(
        caller: &wasmtime::Caller<'_, PluginState>,
        required: &PluginPermission,
    ) -> bool {
        let permissions = &caller.data().permissions;
        if permissions.is_empty() {
            return false;
        }
        permissions.iter().any(|p| matches_permission(p, required))
    }

    fn matches_permission(declared: &PluginPermission, required: &PluginPermission) -> bool {
        match (declared, required) {
            (PluginPermission::Memory, PluginPermission::Memory) => true,
            (PluginPermission::Config, PluginPermission::Config) => true,
            (PluginPermission::Network { hosts }, PluginPermission::Network { .. }) => {
                hosts.is_empty() || hosts.contains(&"*".to_string())
            }
            (PluginPermission::Filesystem { paths }, PluginPermission::Filesystem { .. }) => {
                paths.is_empty() || paths.contains(&"*".to_string())
            }
            (PluginPermission::Env { vars }, PluginPermission::Env { .. }) => {
                vars.is_empty() || vars.contains(&"*".to_string())
            }
            (PluginPermission::System { commands }, PluginPermission::System { .. }) => {
                commands.is_empty() || commands.contains(&"*".to_string())
            }
            _ => false,
        }
    }

    // Consume fuel (non-fatal) on host calls
    fn consume_fuel(caller: &mut wasmtime::Caller<'_, PluginState>, amount: u64) {
        let current = caller.get_fuel().ok();
        if let Some(current) = current {
            let _ = caller.set_fuel(current.saturating_sub(amount));
        }
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
             -> wasmtime::Result<()> {
                consume_fuel(&mut caller, 10);
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
                    })?;
                let data = memory.data(&caller);
                let message = std::str::from_utf8(&data[ptr as usize..(ptr + len) as usize])
                    .unwrap_or("<invalid utf8>");
                info!("[plugin] {}", message);
                Ok(())
            },
        )
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

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
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                if !check_store_permission(&caller, &PluginPermission::Config) {
                    return Ok(0);
                }
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

    // config_get_all(out_ptr, out_len) -> bytes_written
    linker
        .func_wrap(
            "env",
            "config_get_all",
            |mut caller: wasmtime::Caller<'_, PluginState>,
             out_ptr: i32,
             out_len: i32|
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                if !check_store_permission(&caller, &PluginPermission::Config) {
                    return Ok(0);
                }
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

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
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                if !check_store_permission(&caller, &PluginPermission::Memory) {
                    return Ok(0);
                }
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
                    })?;
                let key = read_memory_string(&memory, &mut caller, key_ptr, key_len)?;
                let value: Vec<u8> =
                    memory.data(&caller)[val_ptr as usize..(val_ptr + val_len) as usize].to_vec();
                let state = caller.data();
                let rt = tokio::runtime::Handle::current();
                let mem = state.memory.clone();
                Ok(rt.block_on(async move {
                    mem.write().await.insert(key, value);
                    1i32
                }))
            },
        )
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

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
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                if !check_store_permission(&caller, &PluginPermission::Memory) {
                    return Ok(0);
                }
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
                        mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&data[..to_write]);
                        to_write as i32
                    } else {
                        0i32
                    }
                }))
            },
        )
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

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
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                if !check_store_permission(&caller, &PluginPermission::Memory) {
                    return Ok(0);
                }
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
                    let result = serde_json::to_string(&keys).unwrap_or_else(|_| "[]".to_string());
                    let bytes = result.as_bytes();
                    let to_write = bytes.len().min(out_len);
                    let mem_data = memory.data_mut(&mut caller);
                    mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&bytes[..to_write]);
                    to_write as i32
                }))
            },
        )
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

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
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                if !check_store_permission(&caller, &PluginPermission::Memory) {
                    return Ok(0);
                }
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
                        mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&bytes[..to_write]);
                        to_write as i32
                    } else {
                        0i32
                    }
                }))
            },
        )
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

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
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                if !check_store_permission(&caller, &PluginPermission::Memory) {
                    return Ok(0);
                }
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

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
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                if !check_store_permission(
                    &caller,
                    &PluginPermission::Network { hosts: vec![] },
                ) {
                    let err = r#"{"error":"Permission denied: Network access not granted in plugin manifest"}"#;
                    let bytes = err.as_bytes();
                    let to_write = bytes.len().min(out_len as usize);
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            wasmtime::Error::msg("Plugin does not export a memory segment")
                        })?;
                    memory.data_mut(&mut caller)[out_ptr as usize..out_ptr as usize + to_write]
                        .copy_from_slice(&bytes[..to_write]);
                    return Ok(to_write as i32);
                }
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
                    })?;
                let url = read_memory_string(&memory, &mut caller, url_ptr, url_len)?;
                let out_ptr = out_ptr as usize;
                let out_len = out_len as usize;

                // Record metrics
                let plugin_id = caller.data().plugin_id.clone();
                let metrics_registry = caller.data().shared_state.metrics.clone();
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async {
                    if let Some(m) = metrics_registry.get(&plugin_id).await {
                        m.record_http_request();
                        m.touch();
                    }
                });

                let body = reqwest::blocking::get(&url)
                    .and_then(|r| r.text())
                    .unwrap_or_else(|e| {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(async {
                            if let Some(m) = metrics_registry.get(&plugin_id).await {
                                m.record_http_error();
                            }
                        });
                        format!("{{\"error\":\"{}\"}}", e)
                    });
                let bytes = body.as_bytes();
                let to_write = bytes.len().min(out_len);
                let mem_data = memory.data_mut(&mut caller);
                mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&bytes[..to_write]);
                Ok(to_write as i32)
            },
        )
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

    // http_post(url_ptr, url_len, body_ptr, body_len, content_type_ptr,
    // content_type_len, out_ptr, out_len) -> bytes_written
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
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                if !check_store_permission(
                    &caller,
                    &PluginPermission::Network { hosts: vec![] },
                ) {
                    let err = r#"{"error":"Permission denied: Network access not granted in plugin manifest"}"#;
                    let bytes = err.as_bytes();
                    let to_write = bytes.len().min(out_len as usize);
                    let memory = caller
                        .get_export("memory")
                        .and_then(|e| e.into_memory())
                        .ok_or_else(|| {
                            wasmtime::Error::msg("Plugin does not export a memory segment")
                        })?;
                    memory.data_mut(&mut caller)[out_ptr as usize..out_ptr as usize + to_write]
                        .copy_from_slice(&bytes[..to_write]);
                    return Ok(to_write as i32);
                }
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
                    })?;
                let url = read_memory_string(&memory, &mut caller, url_ptr, url_len)?;
                let body = read_memory_string(&memory, &mut caller, body_ptr, body_len)?;
                let content_type =
                    read_memory_string(&memory, &mut caller, ct_ptr, ct_len)?;
                let out_ptr = out_ptr as usize;
                let out_len = out_len as usize;

                // Record metrics
                let plugin_id = caller.data().plugin_id.clone();
                let metrics_registry = caller.data().shared_state.metrics.clone();
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async {
                    if let Some(m) = metrics_registry.get(&plugin_id).await {
                        m.record_http_request();
                        m.touch();
                    }
                });

                let response = reqwest::blocking::Client::new()
                    .post(&url)
                    .header("Content-Type", &content_type)
                    .body(body)
                    .send()
                    .and_then(|r| r.text())
                    .unwrap_or_else(|e| {
                        let rt = tokio::runtime::Handle::current();
                        rt.block_on(async {
                            if let Some(m) = metrics_registry.get(&plugin_id).await {
                                m.record_http_error();
                            }
                        });
                        format!("{{\"error\":\"{}\"}}", e)
                    });
                let bytes = response.as_bytes();
                let to_write = bytes.len().min(out_len);
                let mem_data = memory.data_mut(&mut caller);
                mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&bytes[..to_write]);
                Ok(to_write as i32)
            },
        )
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

    // --- Events ---

    // emit_event(type_ptr, type_len, payload_ptr, payload_len) -> 1 | 0 (no
    // channel)
    linker
        .func_wrap(
            "env",
            "emit_event",
            |mut caller: wasmtime::Caller<'_, PluginState>,
             type_ptr: i32,
             type_len: i32,
             payload_ptr: i32,
             payload_len: i32|
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

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
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
                        mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&bytes[..to_write]);
                        to_write as i32
                    } else {
                        0i32
                    }
                }))
            },
        )
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

    // get_session_id(out_ptr, out_len) -> bytes_written | 0
    linker
        .func_wrap(
            "env",
            "get_session_id",
            |mut caller: wasmtime::Caller<'_, PluginState>,
             out_ptr: i32,
             out_len: i32|
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
                        mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&bytes[..to_write]);
                        to_write as i32
                    } else {
                        0i32
                    }
                }))
            },
        )
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

    // --- Plugin Info ---

    // get_plugin_id(out_ptr, out_len) -> bytes_written
    linker
        .func_wrap(
            "env",
            "get_plugin_id",
            |mut caller: wasmtime::Caller<'_, PluginState>,
             out_ptr: i32,
             out_len: i32|
             -> wasmtime::Result<i32> {
                consume_fuel(&mut caller, 10);
                let memory = caller
                    .get_export("memory")
                    .and_then(|e| e.into_memory())
                    .ok_or_else(|| {
                        wasmtime::Error::msg("Plugin does not export a memory segment")
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
        .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

    Ok(())
}
