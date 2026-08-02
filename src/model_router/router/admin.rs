//! `ModelRouter` provider/alias/fallback-chain management.

use super::*;

impl ModelRouter {
    // ==================== DEFAULT PROVIDER ====================

    /// Create a default provider (first available)
    pub async fn create_default_provider(&self) -> crate::Result<Arc<dyn Provider + Send + Sync>> {
        let providers = self.providers.read().await;

        if let Some((name, provider)) = providers.iter().next() {
            info!("Using default provider: {name}");
            Ok(provider.clone())
        } else {
            drop(providers);

            if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
                info!("Creating default Anthropic provider from environment");
                let provider =
                    crate::providers::anthropic::AnthropicProvider::new(api_key.clone())?;
                let provider_arc = Arc::new(provider);

                self.auth_profiles
                    .register_single_key("anthropic", api_key)
                    .await;

                let mut providers = self.providers.write().await;
                providers.insert("anthropic".to_string(), provider_arc.clone());

                Ok(provider_arc)
            } else {
                Err(crate::error::ConfigError::Missing(
                    "No providers configured and ANTHROPIC_API_KEY not set".to_string(),
                )
                .into())
            }
        }
    }

    // ==================== ALIAS MANAGEMENT ====================

    /// List available model aliases
    pub async fn list_aliases(&self) -> Vec<String> {
        let config = self.config.read().await;
        config.aliases.keys().cloned().collect()
    }

    /// Add or update a model alias
    pub async fn set_alias(&self, alias: ModelAlias) {
        let mut config = self.config.write().await;
        config.aliases.insert(alias.name.clone(), alias);
    }

    /// Remove a model alias
    pub async fn remove_alias(&self, name: &str) -> bool {
        let mut config = self.config.write().await;
        config.aliases.remove(name).is_some()
    }

    /// Switch the default model alias
    pub async fn switch_default_model(&self, alias_name: &str) -> crate::Result<()> {
        let config = self.config.read().await;
        if !config.aliases.contains_key(alias_name) {
            return Err(crate::error::ConfigError::InvalidValue {
                key: "default_model".to_string(),
                message: format!("Unknown model alias: {alias_name}"),
            }
            .into());
        }
        drop(config);

        let mut config = self.config.write().await;
        info!("Switching default model from '{}' to '{alias_name}'", config.default_model);
        config.default_model = alias_name.to_string();
        Ok(())
    }

    /// Get current default model alias
    pub async fn get_default_model(&self) -> String {
        let config = self.config.read().await;
        config.default_model.clone()
    }

    /// Resolve an alias to its actual model ID.
    pub async fn resolve_alias(&self, alias_name: &str) -> Option<String> {
        let config = self.config.read().await;
        config.aliases.get(alias_name).map(|a| a.model.clone())
    }

    // ==================== PROVIDER MANAGEMENT ====================

    /// List all available providers with their status
    pub async fn list_providers(&self) -> Vec<ProviderInfo> {
        // Narrow lock scopes (Issue 9): collect data while holding each lock,
        // then release before acquiring the next.
        let provider_names: Vec<String> = {
            let providers = self.providers.read().await;
            providers.keys().cloned().collect()
        };

        let health_snapshot: HashMap<String, ProviderHealth> = {
            let health = self.health.read().await;
            health.clone()
        };

        let provider_configs: HashMap<String, ProviderConfig> = {
            let config = self.config.read().await;
            config.providers.clone()
        };

        provider_names
            .into_iter()
            .map(|name| {
                let h = health_snapshot.get(&name).cloned().unwrap_or_default();
                let provider_config = provider_configs.get(&name).cloned();

                ProviderInfo {
                    name: name.clone(),
                    provider_type: provider_config
                        .as_ref()
                        .map(|c| format!("{:?}", c.provider_type))
                        .unwrap_or_default(),
                    enabled: h.state != CircuitState::Open,
                    health: crate::model_router::config::ProviderHealthInfo {
                        state: format!("{:?}", h.state),
                        failures: h.failures,
                        successes: h.successes,
                        avg_latency_ms: h.avg_latency_ms,
                        last_failure: h.last_failure,
                        last_health_check: h.last_health_check,
                    },
                    circuit_state: h.state,
                }
            })
            .collect()
    }

    /// Enable a provider (close circuit breaker)
    pub async fn enable_provider(&self, name: &str) -> crate::Result<()> {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(name) {
            h.state = CircuitState::Closed;
            h.failures = 0;
            info!("Provider {name} enabled (circuit closed)");
            Ok(())
        } else {
            Err(crate::error::ConfigError::InvalidValue {
                key: "provider".to_string(),
                message: format!("Unknown provider: {name}"),
            }
            .into())
        }
    }

    /// Disable a provider (open circuit breaker)
    pub async fn disable_provider(&self, name: &str) -> crate::Result<()> {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(name) {
            h.state = CircuitState::Open;
            info!("Provider {name} disabled (circuit opened)");
            Ok(())
        } else {
            Err(crate::error::ConfigError::InvalidValue {
                key: "provider".to_string(),
                message: format!("Unknown provider: {name}"),
            }
            .into())
        }
    }

    /// Add a new provider at runtime
    pub async fn add_provider(&self, name: &str, config: ProviderConfig) -> crate::Result<()> {
        info!("Adding new provider at runtime: {name}");

        let auth_config = config.derived_auth_profile_config();
        self.auth_profiles
            .register_from_config(name, &auth_config)
            .await;

        let provider = self.create_provider(&config).await?;

        {
            let mut providers = self.providers.write().await;
            providers.insert(name.to_string(), provider);
        }

        {
            let mut health = self.health.write().await;
            health.insert(name.to_string(), ProviderHealth::default());
        }

        {
            let mut router_config = self.config.write().await;
            router_config.providers.insert(name.to_string(), config);
        }

        Ok(())
    }

    /// Add a pre-built provider instance at runtime (e.g. from a plugin).
    pub async fn add_provider_instance(
        &self,
        name: &str,
        provider: Arc<dyn crate::providers::Provider + Send + Sync>,
    ) -> crate::Result<()> {
        info!("Adding provider instance at runtime: {name}");

        {
            let mut providers = self.providers.write().await;
            providers.insert(name.to_string(), provider);
        }

        {
            let mut health = self.health.write().await;
            health.insert(name.to_string(), ProviderHealth::default());
        }

        Ok(())
    }

    /// Remove a provider at runtime
    pub async fn remove_provider(&self, name: &str) -> crate::Result<()> {
        info!("Removing provider at runtime: {name}");

        {
            let mut providers = self.providers.write().await;
            if providers.remove(name).is_none() {
                return Err(crate::error::ConfigError::InvalidValue {
                    key: "provider".to_string(),
                    message: format!("Unknown provider: {name}"),
                }
                .into());
            }
        }

        {
            let mut health = self.health.write().await;
            health.remove(name);
        }

        {
            let mut config = self.config.write().await;
            config.providers.remove(name);
        }

        Ok(())
    }

    /// Get detailed health status for a specific provider
    pub async fn get_provider_health(&self, name: &str) -> Option<ProviderHealthInfo> {
        let health = self.health.read().await;
        health.get(name).map(|h| ProviderHealthInfo {
            state: format!("{:?}", h.state),
            failures: h.failures,
            successes: h.successes,
            avg_latency_ms: h.avg_latency_ms,
            last_failure: h.last_failure,
            last_health_check: h.last_health_check,
        })
    }

    /// Force a health check on a specific provider
    pub async fn check_provider_health(&self, name: &str) -> crate::Result<bool> {
        let provider = {
            let providers = self.providers.read().await;
            providers
                .get(name)
                .cloned()
                .ok_or_else(|| crate::error::ConfigError::InvalidValue {
                    key: "provider".to_string(),
                    message: format!("Unknown provider: {name}"),
                })?
        };

        let start = std::time::Instant::now();
        match provider.health_check().await {
            Ok(true) => {
                self.record_success(name, start.elapsed()).await;
                Ok(true)
            }
            Ok(false) => {
                self.record_failure(name, None).await;
                Ok(false)
            }
            Err(e) => {
                warn!("Health check failed for {}: {}", name, e);
                self.record_failure(name, None).await;
                Ok(false)
            }
        }
    }

    /// Complete a request with a specific provider override
    pub async fn complete_with_provider(
        &self,
        provider_name: &str,
        model: Option<String>,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionResponse> {
        let provider = {
            let providers = self.providers.read().await;
            providers.get(provider_name).cloned().ok_or_else(|| {
                crate::error::ConfigError::InvalidValue {
                    key: "provider".to_string(),
                    message: format!("Unknown provider: {provider_name}"),
                }
            })?
        };

        if self.is_circuit_open(provider_name).await {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Provider {provider_name} circuit is open"),
                cause: None,
            });
        }

        let model_id = model.clone();
        let request = CompletionRequest {
            model,
            messages,
            temperature: None,
            max_tokens: None,
            stream: false,
            tools,
            stop: None,
            extra: None,
            ..Default::default()
        };

        let usage_model = model_id.as_deref().unwrap_or("unknown").to_string();
        let start = std::time::Instant::now();
        match provider.complete(request.clone()).await {
            Ok(response) => {
                self.record_completion_success(
                    provider_name,
                    start.elapsed(),
                    response.usage,
                    &usage_model,
                )
                .await;
                Ok(response)
            }
            Err(ref e) => {
                let class = FailureClass::from_error(e, None);
                warn!("Provider {provider_name} failed with {}: {e}", class.description());

                match self
                    .handle_provider_failure(provider_name, &usage_model, class, e, || {
                        let p = provider.clone();
                        let r = request.clone();
                        async move { p.complete(r).await }
                    })
                    .await
                {
                    Ok(response) => {
                        self.record_completion_success(
                            provider_name,
                            start.elapsed(),
                            response.usage,
                            &usage_model,
                        )
                        .await;
                        Ok(response)
                    }
                    Err(err) => Err(err),
                }
            }
        }
    }

    // ==================== FALLBACK CHAINS ====================

    /// Get fallback chain for an alias
    pub async fn get_fallback_chain(&self, alias_name: &str) -> Vec<String> {
        let chains = self.fallback_chains.read().await;
        chains
            .get(alias_name)
            .map(|entries| entries.iter().map(|e| e.provider.clone()).collect())
            .unwrap_or_default()
    }

    /// Update fallback chain for an alias at runtime
    pub async fn set_fallback_chain(
        &self,
        alias_name: &str,
        provider_chain: Vec<String>,
    ) -> crate::Result<()> {
        let config = self.config.read().await;
        if !config.aliases.contains_key(alias_name) {
            return Err(crate::error::ConfigError::InvalidValue {
                key: "alias".to_string(),
                message: format!("Unknown alias: {alias_name}"),
            }
            .into());
        }
        let model = config
            .aliases
            .get(alias_name)
            .map(|a| a.model.clone())
            .unwrap_or_default();
        drop(config);

        let entries: Vec<FallbackEntry> = provider_chain
            .iter()
            .map(|p| FallbackEntry {
                provider: p.clone(),
                model: model.clone(),
                enabled: true,
                health_score: 100,
            })
            .collect();

        {
            let mut chains = self.fallback_chains.write().await;
            chains.insert(alias_name.to_string(), entries);
        }

        {
            let mut config = self.config.write().await;
            config
                .fallback_chains
                .insert(alias_name.to_string(), provider_chain);
        }

        Ok(())
    }
}
