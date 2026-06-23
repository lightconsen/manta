use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use tracing::{error, info, warn};

use crate::gateway::GatewayState;
use crate::tools::mcp::McpToolWrapper;

// ── Comprehensive reload
// ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ReloadRequest {
    #[serde(default = "default_reload_scope")]
    pub scope: String,
}

fn default_reload_scope() -> String {
    "all".to_string()
}

/// Comprehensive reload handler — reloads plugins, config, providers,
/// MCP servers, and skills without requiring a daemon restart.
pub async fn reload_all_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ReloadRequest>,
) -> impl IntoResponse {
    let scope = req.scope.to_lowercase();
    let mut result = serde_json::json!({ "scope": &scope });

    // ── Snapshot pre-reload config for audit diff ──────────────────────
    let pre_snapshot = state.config.read().await.snapshot();

    // ── 1. Reload main configuration from disk ────────────────────────────
    let new_config =
        if scope == "all" || scope == "config" || scope == "providers" || scope == "mcp" {
            let config_path = state
                .config_path
                .clone()
                .unwrap_or_else(|| crate::dirs::syscity_dir().join("syscity.toml"));

            if config_path.exists() {
                match tokio::fs::read_to_string(&config_path).await {
                    Ok(content) => {
                        match toml::from_str::<crate::gateway::GatewayConfig>(&content) {
                            Ok(cfg) => {
                                info!("Reloaded configuration from {:?}", config_path);
                                Some(cfg)
                            }
                            Err(e) => {
                                error!("Failed to parse syscity.toml: {}", e);
                                None
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to read syscity.toml: {}", e);
                        None
                    }
                }
            } else {
                warn!("Config file not found at {:?}", config_path);
                None
            }
        } else {
            None
        };

    // ── 2. Plugins ────────────────────────────────────────────────────────
    if scope == "all" || scope == "plugins" {
        let plugins = state.infra.plugin_manager.list_plugins().await;
        let ids: Vec<String> = plugins.iter().map(|p| p.id().to_string()).collect();
        let mut unloaded = 0usize;
        for id in &ids {
            match state.infra.plugin_manager.unload_plugin(id).await {
                Ok(_) => unloaded += 1,
                Err(e) => warn!("Failed to unload plugin '{}' during reload: {}", id, e),
            }
        }
        let loaded = match state.infra.plugin_manager.initialize().await {
            Ok(count) => count,
            Err(e) => {
                error!("Failed to initialize plugins: {}", e);
                0
            }
        };
        result["plugins"] = serde_json::json!({
            "unloaded": unloaded,
            "loaded": loaded,
        });
    }

    // ── 3. Config fields (hot-reloadable subset) ──────────────────────────
    if scope == "all" || scope == "config" {
        if let Some(ref new_cfg) = new_config {
            // Validate security/auth config before applying it.
            if let Err(e) = crate::gateway::validate_auth_config(new_cfg) {
                error!("Rejected invalid hot-reload config: {}", e);
                state
                    .auth
                    .audit_log
                    .log(
                        crate::security::runtime_audit::AuditEventType::ConfigChange,
                        "admin",
                        "config",
                        false,
                        format!("Admin reload config rejected: {}", e),
                        None,
                    )
                    .await;
                result["config"] = serde_json::json!({ "updated": false, "reason": e.to_string() });
            } else {
                let mut config_guard = state.config.write().await;
                let config = Arc::make_mut(&mut config_guard);
                config.security = new_cfg.security.clone();
                config.providers = new_cfg.providers.clone();
                config.mcp = new_cfg.mcp.clone();
                config.hot_reload = new_cfg.hot_reload.clone();
                config.cost_guard = new_cfg.cost_guard.clone();
                config.capabilities = new_cfg.capabilities.clone();
                config.computer = new_cfg.computer.clone();
                config.workspace_dir = new_cfg.workspace_dir.clone();
                config.workspace_only = new_cfg.workspace_only;
                config.model = new_cfg.model.clone();
                config.model_provider = new_cfg.model_provider.clone();
                config.dreaming = new_cfg.dreaming.clone();
                config.standing_orders = new_cfg.standing_orders.clone();
                config.cron = new_cfg.cron.clone();
                config.browser = new_cfg.browser.clone();
                config.device = new_cfg.device.clone();
                drop(config_guard);
                result["config"] = serde_json::json!({ "updated": true });
                info!("Applied hot-reloadable configuration fields");
            }
        } else {
            result["config"] =
                serde_json::json!({ "updated": false, "reason": "parse or read error" });
        }
    }

    // ── Compute config diff and log to audit ──────────────────────────
    if scope == "all" || scope == "config" {
        let post_config = state.config.read().await;
        let changes = post_config.diff_since(&pre_snapshot);
        drop(post_config);

        if !changes.is_empty() {
            let details = serde_json::to_value(&changes).unwrap_or_default();
            state
                .auth
                .audit_log
                .log(
                    crate::security::runtime_audit::AuditEventType::ConfigChange,
                    "system",
                    "config",
                    true,
                    format!("Config reloaded: {} field(s) changed", changes.len()),
                    Some(details),
                )
                .await;
            info!(
                changes = ?changes.iter().map(|c| &c.path).collect::<Vec<_>>(),
                "Config changes detected on reload"
            );
        }
    }

    // ── Device subsystem reload (hot-reload drivers from config) ──────
    // Runs when scope is "all", "config", or "device".  Disconnects all
    // existing devices, cleans up old tools, and re-initializes from the
    // new configuration.
    if scope == "all" || scope == "config" || scope == "device" {
        let old_init = state.device_init.write().await.take();

        // Re-scan native plugins directory before reinitializing,
        // so newly placed .so/.dylib files are picked up on hot-reload.
        #[cfg(feature = "native-plugins")]
        {
            let config = state.config.read().await;
            if let Some(ref dir) = config.device.native_plugins_dir {
                tracing::info!("Re-scanning native plugins directory: {:?}", dir);
                state.infra.driver_factory.scan_native_plugins_dir(dir);
            }
        }

        let device_result = {
            // Get the perception registry from state (if initialized)
            let per_init = state.perception_init.read().await;
            let per_reg = per_init.as_ref().map(|pi| &*pi.registry);

            if let Some(old) = old_init {
                let config = state.config.read().await;
                crate::gateway::init::devices::reload_devices(
                    old,
                    &state.infra.driver_factory,
                    &config.device,
                    &state.tools.registry,
                    per_reg,
                    state.task_registry.clone(),
                )
                .await
            } else {
                // No previous init — run fresh init from config
                let config = state.config.read().await;
                let drivers = crate::gateway::init::devices::discover_drivers_from_config(
                    &state.infra.driver_factory,
                    &config.device,
                );
                crate::gateway::init::devices::init_devices(
                    &config.device,
                    drivers,
                    &state.tools.registry,
                    per_reg,
                    &state.task_registry,
                )
                .await
            }
        };

        match device_result {
            Ok(new_init) => {
                // Spawn OS device bridge for the new device init
                let config = state.config.read().await;
                let per_init = state.perception_init.read().await;
                let per_reg = per_init.as_ref().map(|pi| pi.registry.clone());
                if let Some(ref di) = new_init {
                    crate::gateway::init::devices::spawn_os_bridge_from_config(
                        &state.infra.driver_factory,
                        di.registry.clone(),
                        &config.device.os_bridge,
                        state.tools.registry.clone(),
                        per_reg,
                        state.task_registry.clone(),
                    )
                    .await;
                }
                drop(per_init);
                drop(config);
                *state.device_init.write().await = new_init;

                // Abort old control lane handle before re-initializing
                let old_control = state.control_init.write().await.take();
                if let Some(old) = old_control {
                    if let Some(h) = old.handle {
                        h.abort();
                    }
                }

                // Re-init control lane from (possibly updated) config
                let config = state.config.read().await;
                // Re-borrow device_init to get the fresh registry
                let registry = state
                    .device_init
                    .read()
                    .await
                    .as_ref()
                    .map(|di| di.registry.clone());
                drop(config);

                if let Some(reg) = registry {
                    let handlers = crate::device::control::new_handler_registry();
                    let handle = crate::device::control::spawn_control_loop(
                        reg.clone(),
                        handlers.clone(),
                        state.config.clone(),
                    );
                    state
                        .task_registry
                        .insert_abort("control:loop", &handle)
                        .await;
                    *state.control_init.write().await = Some(crate::gateway::state::ControlInit {
                        registry: reg,
                        handle: Some(handle),
                        handlers,
                    });
                    info!("Control lane re-initialized after device reload");
                }

                result["device"] = serde_json::json!({ "reloaded": true });
                info!("Device subsystem reloaded from configuration");
            }
            Err(e) => {
                error!("Failed to reload device subsystem: {}", e);
                result["device"] = serde_json::json!({ "reloaded": false, "error": e.to_string() });
            }
        }
    }

    // ── 4. Providers sync ─────────────────────────────────────────────────
    if scope == "all" || scope == "providers" {
        let (new_providers, current_names) = if let Some(ref new_cfg) = new_config {
            let new_names: std::collections::HashSet<String> =
                new_cfg.providers.keys().cloned().collect();
            let current = state.infra.model_router.list_providers().await;
            let current_names: std::collections::HashSet<String> =
                current.iter().map(|p| p.name.clone()).collect();
            (new_names, current_names)
        } else {
            (std::collections::HashSet::new(), std::collections::HashSet::new())
        };

        let mut added = 0usize;
        let mut removed = 0usize;

        // Remove providers that no longer exist in config
        for name in &current_names {
            if !new_providers.contains(name) {
                if let Err(e) = state.infra.model_router.remove_provider(name).await {
                    warn!("Failed to remove provider '{}': {}", name, e);
                } else {
                    removed += 1;
                    info!("Removed provider '{}' (no longer in config)", name);
                }
            }
        }

        // Add or update providers from new config
        if let Some(ref new_cfg) = new_config {
            for (name, provider_config) in &new_cfg.providers {
                if !current_names.contains(name) {
                    if let Err(e) = state
                        .infra
                        .model_router
                        .add_provider(name, provider_config.clone())
                        .await
                    {
                        warn!("Failed to add provider '{}': {}", name, e);
                    } else {
                        added += 1;
                        info!("Added provider '{}'", name);
                    }
                }
            }
        }

        result["providers"] = serde_json::json!({
            "added": added,
            "removed": removed,
        });
    }

    // ── 5. MCP servers ────────────────────────────────────────────────────
    if scope == "all" || scope == "mcp" {
        // Disconnect all existing MCP servers
        let existing_servers = state.tools.mcp_manager.list_servers().await;
        for server_id in &existing_servers {
            // Deregister tools first
            state
                .tools
                .registry
                .deregister_prefix(&format!("mcp__{}__", server_id));
            if let Err(e) = state.tools.mcp_manager.disconnect(server_id).await {
                warn!("Failed to disconnect MCP server '{}': {}", server_id, e);
            } else {
                info!("Disconnected MCP server '{}'", server_id);
            }
        }

        // Reconnect from new config
        let mut connected = 0usize;
        let mut failed = 0usize;
        if let Some(ref new_cfg) = new_config {
            for (server_id, server_config) in &new_cfg.mcp.servers {
                if !server_config.auto_connect {
                    continue;
                }
                match state
                    .tools
                    .mcp_manager
                    .connect(server_id, server_config.clone())
                    .await
                {
                    Ok(tools) => {
                        info!("MCP server '{}' connected: {} tool(s)", server_id, tools.len());
                        // Register discovered tools
                        if let Some(client_arc) =
                            state.tools.mcp_manager.get_client(server_id).await
                        {
                            let max_tools = if server_config.max_tools == 0 {
                                tools.len()
                            } else {
                                server_config.max_tools.min(tools.len())
                            };
                            for tool in tools.iter().take(max_tools) {
                                let wrapper = Arc::new(McpToolWrapper::new(
                                    client_arc.clone(),
                                    server_id,
                                    tool,
                                ));
                                state.tools.registry.register_dynamic(wrapper);
                            }
                        }
                        connected += 1;
                    }
                    Err(e) => {
                        warn!("Failed to connect MCP server '{}': {}", server_id, e);
                        failed += 1;
                    }
                }
            }
        }

        result["mcp"] = serde_json::json!({
            "disconnected": existing_servers.len(),
            "connected": connected,
            "failed": failed,
        });
    }

    // ── 6. Skills ─────────────────────────────────────────────────────────
    if scope == "all" || scope == "skills" {
        let skills_result = {
            let mut skills_manager = state.tools.skills_manager.write().await;
            match skills_manager.initialize().await {
                Ok(count) => {
                    info!("Reinitialized skills manager with {} skills", count);
                    serde_json::json!({ "reinitialized": true, "count": count })
                }
                Err(e) => {
                    warn!("Failed to reinitialize skills manager: {}", e);
                    serde_json::json!({ "reinitialized": false, "error": e.to_string() })
                }
            }
        };
        result["skills"] = skills_result;
    }

    // ── 7. Channels (document only — rely on file watcher for live reload) ─
    if scope == "all" || scope == "channels" {
        result["channels"] = serde_json::json!({
            "note": "Channels are hot-reloaded automatically when syscity.toml or channel config files change. Use the file watcher or restart individual channels via API.",
        });
    }

    result["success"] = serde_json::json!(true);
    (StatusCode::OK, Json(result)).into_response()
}

// ── Channel management
// ─────────────────────────────────────────────────────────

/// GET /api/v1/channels — List all channels and their enabled status.
pub async fn channel_list_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let config = state.config.read().await;
    let channels: Vec<serde_json::Value> = config
        .channels
        .iter()
        .map(|(name, cfg)| {
            serde_json::json!({
                "name": name,
                "type": cfg.channel_type,
                "enabled": cfg.enabled,
                "agent_id": cfg.agent_id,
            })
        })
        .collect();
    Json(serde_json::json!({ "channels": channels }))
}

/// POST /api/v1/channels/{name}/enable — Enable a channel.
pub async fn enable_channel_handler(
    Path(name): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let mut config_guard = state.config.write().await;
    let channel_config = match Arc::make_mut(&mut config_guard).channels.get_mut(&name) {
        Some(cfg) => cfg,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("Channel '{}' not found", name) })),
            )
                .into_response();
        }
    };

    if channel_config.enabled {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "name": name,
                "enabled": true,
                "message": "Channel is already enabled",
            })),
        )
            .into_response();
    }

    channel_config.enabled = true;
    drop(config_guard);

    // Persist config to disk
    if let Err(e) = persist_config(&state).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to persist config: {}", e) })),
        )
            .into_response();
    }

    info!("Enabled channel '{}' via REST API", name);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "name": name,
            "enabled": true,
            "message": "Channel enabled",
        })),
    )
        .into_response()
}

/// POST /api/v1/channels/{name}/disable — Disable a channel.
pub async fn disable_channel_handler(
    Path(name): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let mut config_guard = state.config.write().await;
    let channel_config = match Arc::make_mut(&mut config_guard).channels.get_mut(&name) {
        Some(cfg) => cfg,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("Channel '{}' not found", name) })),
            )
                .into_response();
        }
    };

    if !channel_config.enabled {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "name": name,
                "enabled": false,
                "message": "Channel is already disabled",
            })),
        )
            .into_response();
    }

    channel_config.enabled = false;
    drop(config_guard);

    // Persist config to disk
    if let Err(e) = persist_config(&state).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to persist config: {}", e) })),
        )
            .into_response();
    }

    info!("Disabled channel '{}' via REST API", name);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "name": name,
            "enabled": false,
            "message": "Channel disabled",
        })),
    )
        .into_response()
}

/// Persist the current gateway config to disk atomically.
async fn persist_config(state: &Arc<GatewayState>) -> Result<(), String> {
    let config_path = match state.config_path.clone() {
        Some(p) => p,
        None => return Err("No config file path configured".to_string()),
    };

    let config = state.config.read().await;
    let toml_str = toml::to_string_pretty(&*config)
        .map_err(|e| format!("TOML serialization failed: {}", e))?;
    drop(config);

    let tmp_path = config_path.with_extension("toml.tmp");
    tokio::fs::write(&tmp_path, toml_str)
        .await
        .map_err(|e| format!("Failed to write temporary config file: {}", e))?;
    tokio::fs::rename(&tmp_path, &config_path)
        .await
        .map_err(|e| format!("Failed to atomically replace config file: {}", e))?;

    info!("Persisted config to {:?}", config_path);
    Ok(())
}
