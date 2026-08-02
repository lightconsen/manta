//! `ModelRouter` completion/streaming request flow with fallback.

use super::*;

impl ModelRouter {
    // ==================== COMPLETION (non-streaming) ====================

    /// Complete a request using the model router
    pub async fn complete(
        &self,
        alias_or_model: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionResponse> {
        let (alias, request) = self
            .build_request(alias_or_model, messages, tools, false)
            .await?;
        let providers_to_try = self.get_providers_to_try(&alias, &request).await;

        self.route_with_fallback(alias, request, providers_to_try, |provider, req| async move {
            provider.complete(req).await
        })
        .await
    }

    /// Stream a completion through the router with fallback and key rotation.
    pub async fn stream(
        &self,
        alias_or_model: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionStream> {
        let (alias, request) = self
            .build_request(alias_or_model, messages, tools, true)
            .await?;
        let providers_to_try = self.get_providers_to_try(&alias, &request).await;

        self.route_with_fallback(alias, request, providers_to_try, |provider, req| async move {
            provider.stream(req).await
        })
        .await
    }

    /// Build a CompletionRequest and resolve the alias.
    async fn build_request(
        &self,
        alias_or_model: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        stream: bool,
    ) -> crate::Result<(ModelAlias, CompletionRequest)> {
        let alias = {
            let config = self.config.read().await;
            config
                .aliases
                .get(alias_or_model)
                .or_else(|| config.aliases.get(&config.default_model))
                .cloned()
                .ok_or_else(|| crate::error::ConfigError::InvalidValue {
                    key: "model_alias".to_string(),
                    message: format!("Unknown model alias: {alias_or_model}"),
                })?
        };

        let request = CompletionRequest {
            model: Some(alias.model.clone()),
            messages,
            temperature: alias.temperature,
            max_tokens: alias.max_tokens,
            stream,
            tools,
            stop: None,
            extra: None,
            requires_vision: false,
            requires_tools: false,
            requires_reasoning: false,
            ..Default::default()
        };

        let alias = self.resolve_alias_with_capabilities(&alias, &request).await;
        Ok((alias, request))
    }

    /// Build the provider chain to try, including fallbacks.
    async fn get_providers_to_try(
        &self,
        alias: &ModelAlias,
        request: &CompletionRequest,
    ) -> Vec<FallbackEntry> {
        let mut providers_to_try = self.get_provider_chain(alias).await;

        for fallback in &request.fallback_models {
            let fb_alias = {
                let config = self.config.read().await;
                config.aliases.get(fallback).cloned()
            };
            if let Some(fb_alias) = fb_alias {
                let fb_chain = self.get_provider_chain(&fb_alias).await;
                for entry in fb_chain {
                    if !providers_to_try
                        .iter()
                        .any(|e| e.provider == entry.provider && e.model == entry.model)
                    {
                        providers_to_try.push(entry);
                    }
                }
            }
        }

        providers_to_try
    }

    /// Generic routing with fallback, circuit breaker, key rotation, and retry.
    ///
    /// Handles the common fallback loop shared by [`complete`] and [`stream`].
    async fn route_with_fallback<T, F, Fut>(
        &self,
        _alias: ModelAlias,
        request: CompletionRequest,
        providers_to_try: Vec<FallbackEntry>,
        provider_fn: F,
    ) -> crate::Result<T>
    where
        T: Send + 'static,
        F: Fn(Arc<dyn Provider>, CompletionRequest) -> Fut + Clone + Send + 'static,
        Fut: std::future::Future<Output = crate::Result<T>> + Send,
    {
        let mut last_error = None;

        for entry in providers_to_try {
            if !entry.enabled {
                continue;
            }

            if self.is_circuit_open(&entry.provider).await {
                warn!("Circuit breaker open for provider: {}", entry.provider);
                continue;
            }

            let provider = {
                let providers = self.providers.read().await;
                providers.get(&entry.provider).cloned()
            };

            if let Some(provider) = provider {
                let start = std::time::Instant::now();
                let provider_clone = provider.clone();
                let request_clone = request.clone();
                let provider_fn_clone = provider_fn.clone();

                match provider_fn(provider, request.clone()).await {
                    Ok(response) => {
                        self.record_success(&entry.provider, start.elapsed()).await;
                        self.auth_profiles.record_success(&entry.provider).await;
                        return Ok(response);
                    }
                    Err(ref e) => {
                        let class = FailureClass::from_error(e, None);
                        warn!(
                            "Provider {} failed with {}: {}",
                            entry.provider,
                            class.description(),
                            e
                        );

                        match self
                            .handle_provider_failure(
                                &entry.provider,
                                &entry.model,
                                class,
                                e,
                                || {
                                    let p = provider_clone.clone();
                                    let r = request_clone.clone();
                                    let f = provider_fn_clone.clone();
                                    async move { f(p, r).await }
                                },
                            )
                            .await
                        {
                            Ok(response) => {
                                self.record_success(&entry.provider, start.elapsed()).await;
                                self.auth_profiles.record_success(&entry.provider).await;
                                return Ok(response);
                            }
                            Err(err) => last_error = Some(err),
                        }
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| crate::error::SyscityError::ExternalService {
            source: "All providers failed".to_string(),
            cause: None,
        }))
    }
}
