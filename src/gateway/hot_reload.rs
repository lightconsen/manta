//! Hot-reload handlers — watch syscity.toml / agent / channel / plugin /
//! gateway configs and apply changes without restarting the process.
//!
//! Extracted from `gateway/mod.rs`. Wired in `Gateway::start()` via
//! `register_hot_reload_handlers(&self.state, self.config.clone(),
//! &hot_reload)`.

use std::sync::Arc;

use tracing::{error, info, warn};

use super::{AgentCommand, ChannelConfig, GatewayConfig, GatewayState};
use crate::agent::AgentConfig;
use crate::config::hot_reload::{ConfigFileType, HotReloadManager};

/// Register hot reload handlers for config changes.
///
/// Registers handlers on the [`HotReloadManager`] for each
/// [`ConfigFileType`] (Main, Agent, Channel, Plugin, Gateway) so that file
/// changes apply to the running gateway state without restart.
pub(crate) async fn register_hot_reload_handlers(
    state: Arc<GatewayState>,
    current_config: GatewayConfig,
    hot_reload: &HotReloadManager,
) {
    // Pre-clone for handlers registered after the main handler
    // (the main handler's `move` closure will consume `state` and `current_config`)
    let state_agent = state.clone();
    let state_channel = state.clone();
    let current_config_channel = current_config.clone();
    let state_plugin = state.clone();
    let state_gateway = state.clone();

    // Handler for main config changes (includes syscity.toml)
    hot_reload
        .register_handler(ConfigFileType::Main, move |_event| {
            let state = state.clone();
            let current_config = current_config.clone();
            async move {
                info!("Main config file changed - reloading configuration");

                // Reload config from disk
                let config_path = crate::dirs::syscity_dir().join("syscity.toml");
                if !config_path.exists() {
                    return Ok(());
                }

                let content = match tokio::fs::read_to_string(&config_path).await {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Failed to read syscity.toml: {}", e);
                        return Ok(());
                    }
                };

                let new_config: GatewayConfig = match toml::from_str(&content) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        error!("Failed to parse syscity.toml: {}", e);
                        return Ok(());
                    }
                };

                // Get current running channels
                let current_channels: Vec<String> = {
                    let channels = state.channels.channels.read().await;
                    channels.keys().cloned().collect()
                };

                // 1. Stop removed or disabled channels
                for name in &current_channels {
                    let should_stop = match new_config.channels.get(name) {
                        None => {
                            info!("Channel '{}' removed from config, stopping...", name);
                            true
                        }
                        Some(cfg) if !cfg.enabled => {
                            info!("Channel '{}' disabled in config, stopping...", name);
                            true
                        }
                        _ => false,
                    };

                    if should_stop {
                        // Remove channel from state (channel will be dropped, should clean up
                        // itself)
                        let removed = {
                            let mut channels = state.channels.channels.write().await;
                            channels.remove(name).is_some()
                        };
                        if removed {
                            info!("✅ Stopped channel '{}'", name);
                        }
                    }
                }

                // 2. Handle new or modified channels
                for (name, new_channel_config) in &new_config.channels {
                    if !new_channel_config.enabled {
                        continue;
                    }

                    let existing = {
                        let channels = state.channels.channels.read().await;
                        channels.get(name).cloned()
                    };

                    match existing {
                        Some(_channel) => {
                            // Channel exists - check if config changed
                            let old_config = current_config.channels.get(name);
                            let config_changed = old_config
                                .map(|old| {
                                    old.credentials != new_channel_config.credentials
                                        || old.allow_from != new_channel_config.allow_from
                                        || old.block_from != new_channel_config.block_from
                                        || old.dm_policy != new_channel_config.dm_policy
                                })
                                .unwrap_or(true);

                            if config_changed {
                                info!("Channel '{}' config changed, restarting...", name);

                                // Remove old channel
                                {
                                    let mut channels = state.channels.channels.write().await;
                                    channels.remove(name);
                                }

                                // Start with new config
                                if let Err(e) = crate::gateway::init::channels::init_single_channel(
                                    state.clone(),
                                    &new_config,
                                    name,
                                    new_channel_config,
                                )
                                .await
                                {
                                    error!("Failed to restart channel '{}': {}", name, e);
                                } else {
                                    info!("✅ Restarted channel '{}' with new config", name);
                                }
                            }
                        }
                        None => {
                            // New channel - initialize it
                            info!(
                                "Hot-reloading new channel: {} ({:?})",
                                name, new_channel_config.channel_type
                            );

                            if let Err(e) = crate::gateway::init::channels::init_single_channel(
                                state.clone(),
                                &new_config,
                                name,
                                new_channel_config,
                            )
                            .await
                            {
                                error!("Failed to hot-reload channel '{}': {}", name, e);
                            } else {
                                info!("✅ Hot-reloaded channel '{}'", name);
                            }
                        }
                    }
                }

                Ok(())
            }
        })
        .await;

    // Handler for agent config changes
    {
        let state = state_agent;
        hot_reload
            .register_handler(ConfigFileType::Agent, move |event| {
                let state = state.clone();
                async move {
                    let agent_name = event
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    info!("Agent config changed for '{}': {:?}", agent_name, event.path);

                    let content = match tokio::fs::read_to_string(&event.path).await {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to read agent config {:?}: {}", event.path, e);
                            return Ok(());
                        }
                    };

                    let new_config: AgentConfig = match toml::from_str(&content) {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to parse agent config for '{}': {}", agent_name, e);
                            return Ok(());
                        }
                    };

                    // Send UpdateConfig to the running agent if it exists
                    let agents = state.agents.agents.read().await;
                    if let Some(handle) = agents.get(&agent_name) {
                        match handle.tx.send(AgentCommand::UpdateConfig(new_config)).await {
                            Ok(_) => {
                                info!("✅ Sent config update to agent '{}'", agent_name)
                            }
                            Err(e) => warn!(
                                "Failed to send config update to agent '{}': {}",
                                agent_name, e
                            ),
                        }
                    } else {
                        info!(
                            "Agent '{}' not currently running; config will apply on next start",
                            agent_name
                        );
                    }

                    Ok(())
                }
            })
            .await;
    }

    // Handler for channel config changes
    {
        let state = state_channel;
        let current_config = current_config_channel;
        hot_reload
            .register_handler(ConfigFileType::Channel, move |event| {
                let state = state.clone();
                let current_config = current_config.clone();
                async move {
                    let channel_name = event
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    info!("Channel config changed for '{}': {:?}", channel_name, event.path);

                    let content = match tokio::fs::read_to_string(&event.path).await {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to read channel config {:?}: {}", event.path, e);
                            return Ok(());
                        }
                    };

                    let new_channel_config: ChannelConfig = match toml::from_str(&content) {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to parse channel config for '{}': {}", channel_name, e);
                            return Ok(());
                        }
                    };

                    if !new_channel_config.enabled {
                        let mut channels = state.channels.channels.write().await;
                        if channels.remove(&channel_name).is_some() {
                            info!("✅ Stopped disabled channel '{}'", channel_name);
                        }
                        return Ok(());
                    }

                    // Stop existing channel
                    {
                        let mut channels = state.channels.channels.write().await;
                        channels.remove(&channel_name);
                    }

                    // Re-initialize with new config
                    match crate::gateway::init::channels::init_single_channel(
                        state.clone(),
                        &current_config,
                        &channel_name,
                        &new_channel_config,
                    )
                    .await
                    {
                        Ok(_) => {
                            info!("✅ Hot-reloaded channel '{}' with updated config", channel_name)
                        }
                        Err(e) => {
                            error!("Failed to hot-reload channel '{}': {}", channel_name, e)
                        }
                    }

                    Ok(())
                }
            })
            .await;
    }

    // Handler for plugin config changes
    {
        let state = state_plugin;
        hot_reload
            .register_handler(ConfigFileType::Plugin, move |event| {
                let state = state.clone();
                async move {
                    let plugin_dir = event.path.parent().unwrap_or(&event.path).to_path_buf();
                    let plugin_id = plugin_dir
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();

                    info!("Plugin file changed for '{}': {:?}", plugin_id, event.path);

                    if plugin_id.is_empty() {
                        warn!("Could not determine plugin ID from path {:?}", event.path);
                        return Ok(());
                    }

                    // Try state-preserving reload first
                    match state.infra.plugin_manager.reload_plugin(&plugin_id).await {
                        Ok(reloaded_id) => {
                            info!("✅ Reloaded plugin '{}' (preserved state)", reloaded_id);
                        }
                        Err(e) => {
                            warn!(
                                "State-preserving reload failed for '{}', falling back to \
                                 unload+load: {}",
                                plugin_id, e
                            );
                            match state.infra.plugin_manager.unload_plugin(&plugin_id).await {
                                Ok(true) => {
                                    match state.infra.plugin_manager.load_plugin(&plugin_dir).await
                                    {
                                        Ok(loaded_id) => {
                                            info!(
                                                "✅ Reloaded plugin '{}' (id={})",
                                                plugin_id, loaded_id
                                            )
                                        }
                                        Err(e) => {
                                            error!("Failed to reload plugin '{}': {}", plugin_id, e)
                                        }
                                    }
                                }
                                Ok(false) => {
                                    match state.infra.plugin_manager.load_plugin(&plugin_dir).await
                                    {
                                        Ok(loaded_id) => {
                                            info!(
                                                "✅ Loaded new plugin '{}' (id={})",
                                                plugin_id, loaded_id
                                            )
                                        }
                                        Err(e) => {
                                            warn!("Could not load plugin '{}': {}", plugin_id, e)
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to unload plugin '{}': {}", plugin_id, e)
                                }
                            }
                        }
                    }

                    Ok(())
                }
            })
            .await;
    }

    // Handler for gateway config changes
    {
        let state = state_gateway;
        hot_reload
            .register_handler(ConfigFileType::Gateway, move |event| {
                let state = state.clone();
                async move {
                    info!("Gateway config changed: {:?}", event.path);

                    // Snapshot before applying changes
                    let pre_snapshot = state.config.read().await.snapshot();

                    let content = match tokio::fs::read_to_string(&event.path).await {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to read gateway config {:?}: {}", event.path, e);
                            return Ok(());
                        }
                    };

                    let new_config: GatewayConfig = match toml::from_str(&content) {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to parse gateway config: {}", e);
                            return Ok(());
                        }
                    };

                    // Apply hot-reloadable fields (those that don't require server restart)
                    let mut config = state.config.write().await;
                    config.security = new_config.security;
                    config.providers = new_config.providers;
                    config.mcp = new_config.mcp;
                    config.hot_reload = new_config.hot_reload;
                    drop(config);
                    info!("✅ Applied gateway config updates (security, providers, mcp settings)");

                    // Compute diff and log to audit
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
                                "file_watcher",
                                "config",
                                true,
                                format!("Hot-reload config change: {} field(s)", changes.len()),
                                Some(details),
                            )
                            .await;
                        info!(
                            changes = ?changes.iter().map(|c| &c.path).collect::<Vec<_>>(),
                            "Gateway config hot-reload changes"
                        );
                    }

                    Ok(())
                }
            })
            .await;
    }

    info!("Registered hot reload handlers for all config types");
}
