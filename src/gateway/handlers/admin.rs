use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use tracing::{error, info, warn};

use crate::gateway::GatewayState;
use crate::mcp::McpToolWrapper;

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
/// `GET /api/v1/models` — list available concrete model IDs.
pub async fn list_models_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let entries = state.infra.model_router.model_catalog.list().await;
    Json(serde_json::json!({
        "models": entries,
    }))
}

/// `GET /api/v1/models/default` — the current default model.
pub async fn get_default_model_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let default = state.infra.model_router.get_default_model().await;
    Json(serde_json::json!({
        "default_model": default,
    }))
}

/// `GET /v1/models` — available model IDs in OpenAI wire format.
pub async fn openai_list_models_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let entries = state.infra.model_router.model_catalog.list().await;
    let data: Vec<_> = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.id.clone(),
                "object": "model",
                "created": 0,
                "owned_by": entry.provider.clone(),
            })
        })
        .collect();
    Json(serde_json::json!({ "object": "list", "data": data }))
}

pub async fn reload_all_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ReloadRequest>,
) -> impl IntoResponse {
    let scope = req.scope.to_lowercase();
    let mut result = serde_json::json!({ "scope": &scope });

    // Take one consistent config snapshot for the entire reload operation.
    // All downstream code uses this snapshot instead of re-locking config,
    // preventing interleaved config states from hot-reload or other requests.
    let config_snapshot = Arc::new(state.config.read().await.clone());

    // ── Snapshot pre-reload config for audit diff ──────────────────────
    let pre_snapshot = config_snapshot.snapshot();

    // ── 1. Reload main configuration from disk ────────────────────────────
    let new_config =
        if scope == "all" || scope == "config" || scope == "providers" || scope == "mcp" {
            let config_path = state
                .config_path
                .clone()
                .unwrap_or_else(crate::dirs::default_config_file);

            if config_path.exists() {
                match tokio::fs::read_to_string(&config_path).await {
                    Ok(content) => {
                        match toml::from_str::<crate::gateway::GatewayConfig>(&content) {
                            Ok(cfg) => {
                                info!("Reloaded configuration from {:?}", config_path);
                                Some(cfg)
                            }
                            Err(e) => {
                                error!("Failed to parse config.toml: {}", e);
                                None
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to read config.toml: {}", e);
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

    // Use the freshly reloaded config for this operation, falling back to the
    // consistent snapshot so all scopes see the same state.
    let _active_config = new_config.as_ref().unwrap_or(&config_snapshot);

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
                #[cfg(feature = "browser")]
                {
                    config.browser = new_cfg.browser.clone();
                }
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
            "note": "Channels are hot-reloaded automatically when config.toml or channel config files change. Use the file watcher or restart individual channels via API.",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::ChannelType;
    use crate::gateway::config::ChannelConfig;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    async fn state() -> Arc<GatewayState> {
        Arc::new(make_test_state(GatewayConfig::default()).await)
    }

    /// Test state whose config contains a single telegram channel with the
    /// given `enabled` state.
    async fn state_with_channel(enabled: bool) -> Arc<GatewayState> {
        let state = state().await;
        {
            let mut config_guard = state.config.write().await;
            let config = Arc::make_mut(&mut config_guard);
            let mut ch = ChannelConfig::new(ChannelType::Telegram);
            ch.enabled = enabled;
            config.channels.insert("telegram".to_string(), ch);
        }
        state
    }

    #[tokio::test]
    async fn reload_channels_scope_ok() {
        let state = state().await;
        let body = Json(ReloadRequest { scope: "channels".into() });
        let (status, json) =
            body_json(reload_all_handler(State(state), body).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], true);
        assert!(json["channels"]["note"].is_string());
    }

    #[tokio::test]
    async fn reload_skills_scope_ok() {
        let state = state().await;
        let body = Json(ReloadRequest { scope: "skills".into() });
        let (status, json) =
            body_json(reload_all_handler(State(state), body).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], true);
        assert!(json["skills"].is_object());
    }

    #[tokio::test]
    async fn channel_list_empty_ok() {
        let state = state().await;
        let (status, json) =
            body_json(channel_list_handler(State(state)).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["channels"].is_array());
        assert!(json["channels"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn channel_list_with_entry_reports_fields() {
        let state = state_with_channel(true).await;
        let (status, json) =
            body_json(channel_list_handler(State(state)).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        let channels = json["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["name"], "telegram");
        assert_eq!(channels[0]["enabled"], true);
    }

    #[tokio::test]
    async fn enable_channel_unknown_404() {
        let state = state().await;
        let (status, json) = body_json(
            enable_channel_handler(Path("telegram".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(json["error"].as_str().unwrap().contains("telegram"));
    }

    #[tokio::test]
    async fn disable_channel_unknown_404() {
        let state = state().await;
        let (status, json) = body_json(
            disable_channel_handler(Path("telegram".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(json["error"].as_str().unwrap().contains("telegram"));
    }

    #[tokio::test]
    async fn enable_already_enabled_ok() {
        let state = state_with_channel(true).await;
        let (status, json) = body_json(
            enable_channel_handler(Path("telegram".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["message"], "Channel is already enabled");
    }

    #[tokio::test]
    async fn disable_already_disabled_ok() {
        let state = state_with_channel(false).await;
        let (status, json) = body_json(
            disable_channel_handler(Path("telegram".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["message"], "Channel is already disabled");
    }

    #[tokio::test]
    async fn enable_channel_persist_fails_500() {
        // Test state has no config_path, so flipping the enabled flag trips the
        // persist step, which errors out.
        let state = state_with_channel(false).await;
        let (status, json) = body_json(
            enable_channel_handler(Path("telegram".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("Failed to persist config"));
    }

    #[tokio::test]
    async fn disable_channel_persist_fails_500() {
        let state = state_with_channel(true).await;
        let (status, json) = body_json(
            disable_channel_handler(Path("telegram".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("Failed to persist config"));
    }
}
