//! config.get / config.set over WebSocket.

use super::*;
pub(super) async fn handle_config_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let config = state.config.read().await;
    let heartbeat = &config.heartbeat;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "model": config.model,
            "model_provider": config.model_provider,
            "default_agent": {
                "temperature": config.default_agent.temperature,
                "max_tokens": config.default_agent.max_tokens,
                "max_turns": config.default_agent.max_turns,
                "max_concurrent_tools": config.default_agent.max_concurrent_tools,
                "system_prompt": config.default_agent.system_prompt,
                "workspace_only": config.default_agent.workspace_only,
            },
            "heartbeat": {
                "enabled": heartbeat.enabled,
                "interval_seconds": heartbeat.interval_seconds,
                "active_hours_start": heartbeat.active_hours_start,
                "active_hours_end": heartbeat.active_hours_end,
                "max_consecutive_idle": heartbeat.max_consecutive_idle,
            },
            "channels": config.channels.iter().map(|(k, v)| {
                serde_json::json!({
                    "name": k,
                    "channel_type": format!("{:?}", v.channel_type).to_lowercase(),
                    "enabled": v.enabled,
                    "agent_id": v.agent_id,
                    "dm_policy": format!("{:?}", v.dm_policy).to_lowercase(),
                    "require_mention": v.require_mention,
                    "has_credentials": !v.credentials.is_empty(),
                })
            }).collect::<Vec<_>>(),
            "auth_mode": config.security.auth_mode,
            "search": {
                "provider": config.search.provider,
                "providers": config.search.providers,
                "has_api_key": !config.search.api_key.is_empty(),
                "keys": {
                    "tavily": (!config.search.keys.get("tavily").is_none_or(|k| k.is_empty())).to_string(),
                    "serpapi": (!config.search.keys.get("serpapi").is_none_or(|k| k.is_empty())).to_string(),
                    "exa": (!config.search.keys.get("exa").is_none_or(|k| k.is_empty())).to_string(),
                    "firecrawl": (!config.search.keys.get("firecrawl").is_none_or(|k| k.is_empty())).to_string(),
                    "bing": (!config.search.keys.get("bing").is_none_or(|k| k.is_empty())).to_string(),
                    "google": (!config.search.keys.get("google").is_none_or(|k| k.is_empty())).to_string(),
                    "google_cx": (!config.search.keys.get("google_cx").is_none_or(|k| k.is_empty())).to_string(),
                    "brave": (!config.search.keys.get("brave").is_none_or(|k| k.is_empty())).to_string(),
                },
            },
        }),
    )
}

pub(super) async fn handle_config_set(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct SetParams {
        path: String,
        value: serde_json::Value,
    }

    let params: SetParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    // Handle model switching outside the config write lock so the lock is not
    // held across an async model-router operation.
    let model_update = if params.path == "model" {
        if let Some(v) = params.value.as_str() {
            match state.infra.model_router.switch_default_model(v).await {
                Ok(()) => Some(v.to_string()),
                Err(e) => {
                    return WsResponse::err(
                        &req.id,
                        "CONFIG_ERROR",
                        format!("Failed to switch model: {}", e),
                    );
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let mut config_guard = state.config.write().await;
    let config = Arc::make_mut(&mut config_guard);

    match params.path.as_str() {
        "model" => {
            if let Some(v) = model_update {
                config.model = v;
            }
        }
        "model_provider" => {
            if let Some(v) = params.value.as_str() {
                config.model_provider = v.to_string();
            }
        }
        "default_agent.temperature" => {
            if let Some(v) = params.value.as_f64() {
                config.default_agent.temperature = v as f32;
            }
        }
        "default_agent.max_tokens" => {
            if let Some(v) = params.value.as_u64() {
                config.default_agent.max_tokens = v as u32;
            }
        }
        "default_agent.max_turns" => {
            config.default_agent.max_turns = params.value.as_u64().map(|v| v as usize);
        }
        "default_agent.max_concurrent_tools" => {
            if let Some(v) = params.value.as_u64() {
                config.default_agent.max_concurrent_tools = v as usize;
            }
        }
        "default_agent.system_prompt" => {
            if let Some(v) = params.value.as_str() {
                config.default_agent.system_prompt = v.to_string();
            }
        }
        "default_agent.workspace_only" => {
            if let Some(v) = params.value.as_bool() {
                config.default_agent.workspace_only = v;
            }
        }
        "heartbeat.enabled" => {
            if let Some(v) = params.value.as_bool() {
                config.heartbeat.enabled = v;
            }
        }
        "heartbeat.interval_seconds" => {
            if let Some(v) = params.value.as_u64() {
                config.heartbeat.interval_seconds = v;
            }
        }
        "heartbeat.active_hours_start" => {
            if let Some(v) = params.value.as_str() {
                config.heartbeat.active_hours_start = v.to_string();
            }
        }
        "heartbeat.active_hours_end" => {
            if let Some(v) = params.value.as_str() {
                config.heartbeat.active_hours_end = v.to_string();
            }
        }
        "heartbeat.max_consecutive_idle" => {
            if let Some(v) = params.value.as_u64() {
                config.heartbeat.max_consecutive_idle = v as u32;
            }
        }
        "channels.add" => {
            #[derive(Debug, Deserialize)]
            struct ChannelAddPayload {
                name: String,
                channel_type: String,
                enabled: Option<bool>,
                agent_id: Option<String>,
                credentials: Option<HashMap<String, String>>,
            }
            let payload: ChannelAddPayload = match serde_json::from_value(params.value) {
                Ok(p) => p,
                Err(e) => return WsResponse::err(&req.id, "INVALID_PARAMS", e.to_string()),
            };
            let channel_type = match payload.channel_type.as_str() {
                "telegram" => crate::channels::ChannelType::Telegram,
                "discord" => crate::channels::ChannelType::Discord,
                "slack" => crate::channels::ChannelType::Slack,
                "whatsapp" => crate::channels::ChannelType::Whatsapp,
                "qq" => crate::channels::ChannelType::Qq,
                "feishu" => crate::channels::ChannelType::Feishu,
                "signal" => crate::channels::ChannelType::Signal,
                "imessage" => crate::channels::ChannelType::Imessage,
                "webchat" => crate::channels::ChannelType::Webchat,
                "websocket" => crate::channels::ChannelType::Websocket,
                "web_terminal" => crate::channels::ChannelType::WebTerminal,
                other => {
                    return WsResponse::err(
                        &req.id,
                        "INVALID_CHANNEL_TYPE",
                        format!("Unknown channel type: {}", other),
                    )
                }
            };
            let mut ch = crate::gateway::ChannelConfig::new(channel_type);
            if let Some(v) = payload.enabled {
                ch.enabled = v;
            }
            if let Some(v) = payload.agent_id {
                ch.agent_id = Some(v);
            }
            if let Some(v) = payload.credentials {
                ch.credentials = v;
            }
            config.channels.insert(payload.name.clone(), ch);
        }
        "channels.update" => {
            #[derive(Debug, Deserialize)]
            struct ChannelUpdatePayload {
                name: String,
                enabled: Option<bool>,
                agent_id: Option<String>,
                credentials: Option<HashMap<String, String>>,
            }
            let payload: ChannelUpdatePayload = match serde_json::from_value(params.value) {
                Ok(p) => p,
                Err(e) => return WsResponse::err(&req.id, "INVALID_PARAMS", e.to_string()),
            };
            match config.channels.get_mut(&payload.name) {
                Some(ch) => {
                    if let Some(v) = payload.enabled {
                        ch.enabled = v;
                    }
                    if let Some(v) = payload.agent_id {
                        ch.agent_id = Some(v);
                    }
                    if let Some(v) = payload.credentials {
                        ch.credentials = v;
                    }
                }
                None => {
                    return WsResponse::err(
                        &req.id,
                        "CHANNEL_NOT_FOUND",
                        format!("Channel '{}' not found", payload.name),
                    )
                }
            }
        }
        "channels.remove" => {
            if let Some(name) = params.value.as_str() {
                config.channels.remove(name);
            } else {
                return WsResponse::err(&req.id, "INVALID_PARAMS", "Expected channel name string");
            }
        }
        "channels.set_enabled" => {
            #[derive(Debug, Deserialize)]
            struct SetEnabledPayload {
                name: String,
                enabled: bool,
            }
            let payload: SetEnabledPayload = match serde_json::from_value(params.value) {
                Ok(p) => p,
                Err(e) => return WsResponse::err(&req.id, "INVALID_PARAMS", e.to_string()),
            };
            match config.channels.get_mut(&payload.name) {
                Some(ch) => ch.enabled = payload.enabled,
                None => {
                    return WsResponse::err(
                        &req.id,
                        "CHANNEL_NOT_FOUND",
                        format!("Channel '{}' not found", payload.name),
                    )
                }
            }
        }
        "search.provider" => {
            if let Some(v) = params.value.as_str() {
                config.search.provider = v.to_string();
            }
        }
        "search.providers" => {
            if let Some(arr) = params.value.as_array() {
                config.search.providers = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }
        }
        _ if params.path.starts_with("search.keys.") => {
            let key_name = params.path.strip_prefix("search.keys.").unwrap_or("");
            if !key_name.is_empty() {
                match &params.value {
                    serde_json::Value::String(v) if !v.is_empty() => {
                        config.search.keys.insert(key_name.to_string(), v.clone());
                    }
                    _ => {
                        config.search.keys.remove(key_name);
                    }
                }
            }
        }
        _ => {
            return WsResponse::err(
                &req.id,
                "UNKNOWN_CONFIG_PATH",
                format!("Unknown config path: {}", params.path),
            );
        }
    }

    // Mirror sensitive channel credentials into the secret store so a store
    // copy always exists (the plaintext credentials map stays for backward
    // compatibility until `secrets migrate` strips it).
    for (id, channel_config) in config.channels.iter() {
        if let Err(e) =
            crate::secrets::persist_channel_secrets(id, &channel_config.credentials).await
        {
            tracing::warn!("Failed to persist channel secrets for '{}': {}", id, e);
        }
    }

    // Persist config to disk so changes survive restarts and trigger hot-reload.
    // Keep the write lock held across persistence so concurrent writers cannot
    // overwrite our update before it is serialized.
    if let Some(config_path) = state.config_path.clone() {
        if let Err(e) = persist_config_atomic(&config_guard, &config_path).await {
            return WsResponse::err(
                &req.id,
                "PERSIST_FAILED",
                format!("Config updated in memory but failed to persist: {}", e),
            );
        }
    }
    drop(config_guard);

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "updated",
            "path": params.path,
        }),
    )
}
