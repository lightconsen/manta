//! models.list / presets / fetch_remote / add / remove / set_default.

use super::*;
pub(super) async fn handle_models_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    // Build model list from aliases (always available) rather than catalog
    // which may be empty if initialize() was never called.
    let aliases = state.infra.model_router.aliases_with_configs().await;
    let entries: Vec<serde_json::Value> = aliases
        .iter()
        .map(|(name, alias)| {
            serde_json::json!({
                "id": name,
                "name": format!("{} ({})", name, alias.model),
                "provider": alias.provider,
            })
        })
        .collect();
    let default_model = state.infra.model_router.get_default_model().await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "models": entries,
            "default_model": default_model,
        }),
    )
}

pub(super) async fn handle_models_presets(
    req: &WsRequest,
    _state: &Arc<GatewayState>,
) -> WsResponse {
    let presets = crate::model_router::provider_presets();
    let builtins = crate::providers::preset::builtin_providers();
    let list: Vec<serde_json::Value> = presets
        .into_iter()
        .map(|(name, p)| {
            // Enrich with protocol/auth info from the TOML registry when the
            // preset exists there (custom does not).
            let builtin = builtins.get(name.as_str());
            let protocol = builtin.and_then(|b| b.variants.first()).map(|v| v.protocol);
            let needs_api_key = builtin
                .and_then(|b| b.variants.first())
                .map(|v| v.auth_method != crate::providers::AuthMethod::None)
                .unwrap_or(true);
            // Fall back to the TOML registry base URL when the legacy preset
            // does not define one (e.g. Anthropic, Gemini).
            let base_url = p.default_base_url.or_else(|| {
                builtin
                    .and_then(|b| b.variants.first())
                    .map(|v| v.default_base_url.clone())
            });
            serde_json::json!({
                "name": name,
                "display_name": p.display_name,
                "base_url": base_url,
                "models": p.models,
                "protocol": protocol,
                "needs_api_key": needs_api_key,
            })
        })
        .collect();
    WsResponse::ok(&req.id, serde_json::json!({ "presets": list }))
}

/// Build the list-models endpoint URL for a protocol.
pub(super) fn models_endpoint_url(protocol: crate::providers::Protocol, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match protocol {
        crate::providers::Protocol::OpenAi => format!("{base}/models"),
        crate::providers::Protocol::Anthropic => format!("{base}/v1/models"),
        crate::providers::Protocol::Gemini => format!("{base}/models"),
    }
}

/// Parse model IDs from an OpenAI/Anthropic-style `{ "data": [{ "id": ... }] }` body.
pub(super) fn parse_data_models(body: &serde_json::Value) -> Vec<String> {
    body.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse model IDs from a Gemini `{ "models": [{ "name": "models/..." }] }` body.
pub(super) fn parse_gemini_models(body: &serde_json::Value) -> Vec<String> {
    body.get("models")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                .map(|n| n.strip_prefix("models/").unwrap_or(n).to_string())
                .collect()
        })
        .unwrap_or_default()
}

pub(super) async fn handle_models_fetch_remote(
    req: &WsRequest,
    _state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct FetchRemotePayload {
        provider: String,
        base_url: Option<String>,
        api_key: Option<String>,
        /// Protocol override, required for providers not in the registry.
        protocol: Option<crate::providers::Protocol>,
    }
    let payload: FetchRemotePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    // Resolve protocol / default base URL / auth method from the TOML registry.
    let builtins = crate::providers::preset::builtin_providers();
    let variant = builtins
        .get(payload.provider.as_str())
        .and_then(|b| b.variants.first());

    let protocol = match payload.protocol.or_else(|| variant.map(|v| v.protocol)) {
        Some(p) => p,
        None => {
            return WsResponse::err(
                &req.id,
                "PROTOCOL_REQUIRED",
                format!(
                    "Unknown provider '{}'; an explicit protocol is required",
                    payload.provider
                ),
            );
        }
    };

    let base_url = match payload
        .base_url
        .filter(|u| !u.is_empty())
        .or_else(|| variant.map(|v| v.default_base_url.clone()))
    {
        Some(u) => u,
        None => {
            return WsResponse::err(
                &req.id,
                "BASE_URL_REQUIRED",
                format!("Provider '{}' requires a base_url", payload.provider),
            );
        }
    };

    if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
        return WsResponse::err(
            &req.id,
            "INVALID_BASE_URL",
            "base_url must start with http:// or https://".to_string(),
        );
    }

    let auth_method = variant
        .map(|v| v.auth_method.clone())
        .unwrap_or(crate::providers::AuthMethod::Bearer);
    let api_key = payload.api_key.filter(|k| !k.is_empty());

    let static_fallback = || {
        crate::model_router::provider_presets()
            .get(&payload.provider)
            .map(|p| p.models.clone())
            .unwrap_or_default()
    };

    let url = match variant.and_then(|v| v.models_endpoint.as_deref()) {
        Some(endpoint) => format!("{}{}", base_url.trim_end_matches('/'), endpoint),
        None => models_endpoint_url(protocol, &base_url),
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "models": static_fallback(),
                    "source": "static",
                    "error": format!("HTTP client error: {e}"),
                }),
            );
        }
    };

    let mut request = client.get(&url);
    match (&auth_method, &api_key) {
        (crate::providers::AuthMethod::Bearer, Some(key)) => {
            request = request.bearer_auth(key);
        }
        (crate::providers::AuthMethod::ApiKeyHeader, Some(key)) => {
            request = request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01");
        }
        (crate::providers::AuthMethod::GoogleApiKey, Some(key)) => {
            request = request.header("x-goog-api-key", key);
        }
        (crate::providers::AuthMethod::CustomHeader { name }, Some(key)) => {
            request = request.header(name, key);
        }
        _ => {}
    }

    match request.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => {
                let models = match protocol {
                    crate::providers::Protocol::Gemini => parse_gemini_models(&body),
                    _ => parse_data_models(&body),
                };
                if models.is_empty() {
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "models": static_fallback(),
                            "source": "static",
                            "error": "Provider returned an empty model list",
                        }),
                    )
                } else {
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({ "models": models, "source": "remote" }),
                    )
                }
            }
            Err(e) => WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "models": static_fallback(),
                    "source": "static",
                    "error": format!("Failed to parse provider response: {e}"),
                }),
            ),
        },
        Ok(resp) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "models": static_fallback(),
                "source": "static",
                "error": format!("Provider returned HTTP {}", resp.status()),
            }),
        ),
        Err(e) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "models": static_fallback(),
                "source": "static",
                "error": format!("Failed to reach provider: {e}"),
            }),
        ),
    }
}

pub(super) async fn handle_models_add(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct ModelAddPayload {
        name: String,
        provider: String,
        model: String,
        api_key: Option<String>,
        base_url: Option<String>,
    }
    let payload: ModelAddPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let provider_name = payload.provider.clone();
    let presets = crate::model_router::provider_presets();
    let preset = presets.get(&provider_name);

    // If api_key provided, configure or update the provider
    if let Some(api_key) = payload.api_key.filter(|k| !k.is_empty()) {
        let (provider_type, base_url) = match preset {
            Some(p) => (
                p.protocol.clone(),
                payload
                    .base_url
                    .clone()
                    .or_else(|| p.default_base_url.clone()),
            ),
            None => (
                crate::model_router::ProviderType::Custom { name: provider_name.clone() },
                payload.base_url.clone(),
            ),
        };

        let provider_config = crate::model_router::ProviderConfig {
            provider_type,
            api_key: api_key.clone().into(),
            api_keys: Vec::new(),
            auth_profile: None,
            oauth: None,
            base_url,
            timeout: std::time::Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        // Update GatewayConfig providers
        {
            let mut config_guard = state.config.write().await;
            Arc::make_mut(&mut config_guard)
                .providers
                .insert(provider_name.clone(), provider_config.clone());
        }

        // Register with model router
        if let Err(e) = state
            .infra
            .model_router
            .add_provider(&provider_name, provider_config)
            .await
        {
            return WsResponse::err(
                &req.id,
                "PROVIDER_ERROR",
                format!("Failed to register provider: {}", e),
            );
        }
    }

    // Set alias
    let alias = crate::model_router::ModelAlias {
        name: payload.name.clone(),
        provider: provider_name,
        model: payload.model,
        temperature: None,
        max_tokens: None,
    };
    state.infra.model_router.set_alias(alias).await;

    // If this is the first alias, auto-set it as default
    let aliases = state.infra.model_router.list_aliases().await;
    if aliases.len() == 1 {
        if let Err(e) = state
            .infra
            .model_router
            .switch_default_model(&payload.name)
            .await
        {
            warn!("Failed to switch default model to {}: {}", payload.name, e);
        }
    }

    // Register in catalog for discovery
    let entry = crate::model_router::ModelCatalogEntry::new(
        payload.name.clone(),
        format!("{} ({})", payload.name, payload.name),
        payload.name.clone(),
    )
    .with_alias(payload.name.clone());
    state.infra.model_router.model_catalog.register(entry).await;

    // Persist GatewayConfig to config.toml
    if let Some(config_path) = state.config_path.clone() {
        let config_guard = state.config.read().await;
        if let Err(e) = persist_config_atomic(&config_guard, &config_path).await {
            return WsResponse::err(
                &req.id,
                "PERSIST_FAILED",
                format!("Model added but failed to persist config: {}", e),
            );
        }
    }

    WsResponse::ok(&req.id, serde_json::json!({ "status": "added" }))
}

pub(super) async fn handle_models_remove(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct RemovePayload {
        name: String,
    }
    let payload: RemovePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let removed = state.infra.model_router.remove_alias(&payload.name).await;
    if removed {
        WsResponse::ok(&req.id, serde_json::json!({ "status": "removed" }))
    } else {
        WsResponse::err(
            &req.id,
            "MODEL_NOT_FOUND",
            format!("Model alias '{}' not found", payload.name),
        )
    }
}

pub(super) async fn handle_models_set_default(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct SetDefaultPayload {
        name: String,
    }
    let payload: SetDefaultPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state
        .infra
        .model_router
        .switch_default_model(&payload.name)
        .await
    {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "status": "ok", "default_model": payload.name }),
        ),
        Err(e) => WsResponse::err(&req.id, "MODEL_NOT_FOUND", format!("{}", e)),
    }
}
