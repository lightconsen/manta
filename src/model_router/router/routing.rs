//! `ModelRouter` completion/streaming request flow with fallback.

use super::*;

impl ModelRouter {
    // ==================== COMPLETION (non-streaming) ====================

    /// Complete a request using the model router
    pub async fn complete(
        &self,
        model_id: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionResponse> {
        let (provider, model_id, request) =
            self.build_request(model_id, messages, tools, false).await?;
        let providers_to_try = self
            .get_providers_to_try(&provider, &model_id, &request)
            .await;

        self.route_with_fallback(&model_id, request, providers_to_try, |provider, req| async move {
            provider.complete(req).await
        })
        .await
    }

    /// Stream a completion through the router with fallback and key rotation.
    pub async fn stream(
        &self,
        model_id: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionStream> {
        let (provider, model_id, request) =
            self.build_request(model_id, messages, tools, true).await?;
        let providers_to_try = self
            .get_providers_to_try(&provider, &model_id, &request)
            .await;

        self.route_with_fallback(&model_id, request, providers_to_try, |provider, req| async move {
            provider.stream(req).await
        })
        .await
    }

    /// Build a CompletionRequest and resolve the model to its provider.
    /// Returns `(provider, model_id, request)`. Unknown models fall back to
    /// the global default model.
    async fn build_request(
        &self,
        model_id: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
        stream: bool,
    ) -> crate::Result<(String, String, CompletionRequest)> {
        let (provider, resolved_model) = {
            if let Some(provider) = self.provider_for_model(model_id).await {
                (provider, model_id.to_string())
            } else {
                let default_model = self.get_default_model().await;
                let provider = self
                    .provider_for_model(&default_model)
                    .await
                    .ok_or_else(|| crate::error::ConfigError::InvalidValue {
                        key: "model".to_string(),
                        message: format!("Unknown model: {model_id}"),
                    })?;
                (provider, default_model)
            }
        };

        let request = CompletionRequest {
            model: Some(resolved_model.clone()),
            messages,
            temperature: None,
            max_tokens: None,
            stream,
            tools,
            stop: None,
            extra: None,
            requires_vision: false,
            requires_tools: false,
            requires_reasoning: false,
            ..Default::default()
        };

        let (provider, resolved_model) = self
            .resolve_model_with_capabilities(&provider, &resolved_model, &request)
            .await;
        Ok((provider, resolved_model, request))
    }

    /// Build the provider chain to try, including fallbacks.
    async fn get_providers_to_try(
        &self,
        provider: &str,
        model_id: &str,
        request: &CompletionRequest,
    ) -> Vec<FallbackEntry> {
        let mut providers_to_try = self.get_provider_chain(provider, model_id).await;

        for fallback in &request.fallback_models {
            if let Some(fb_provider) = self.provider_for_model(fallback).await {
                let fb_chain = self.get_provider_chain(&fb_provider, fallback).await;
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
        _model_id: &str,
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
