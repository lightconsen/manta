//! ModelRouter — multi-provider LLM routing with fallback chains
//!
//! Provides the primary [`ModelRouter`] struct with support for:
//! - Model aliases (e.g. "fast" → "claude-3-haiku")
//! - Multi-provider routing with automatic fallback
//! - Health checking and circuit breaker
//! - Auth profile / API key rotation
//! - Cost-aware routing with task classification
//! - Pluggable task classifier

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::future::join_all;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::gateway::task_registry::TaskRegistry;
use crate::model_router::auth_profile::{AuthProfileManager, ProfileStatus};
use crate::model_router::classifier::{KeywordTaskClassifier, TaskClassifierImpl};
use crate::model_router::config::{
    CircuitState, CostAwareConfig, FallbackEntry, ModelAlias, ModelRouterConfig, ProviderConfig,
    ProviderHealth, ProviderHealthInfo, ProviderInfo, ProviderType, TaskType,
};
use crate::model_router::failure_class::FailureClass;
use crate::model_router::model_catalog::{self, ModelCatalog, ModelCatalogEntry};
use crate::model_router::oauth_credential::Credential;
use crate::model_router::usage_fetcher::{
    LocalBudgetFetcher, OpenAiUsageFetcher, UsageFetcher, UsageFetcherRegistry,
};
use crate::model_router::usage_tracker::{
    ProviderUsageSnapshot, ProviderUsageTracker, UsageQuota, UsageTrackerConfig,
};
use crate::providers::{
    CompletionRequest, CompletionResponse, CompletionStream, Message, Provider, ToolDefinition,
    Usage,
};

// ------------------------------------------------------------------
// ModelRouter
// ------------------------------------------------------------------

/// Model router for multi-provider LLM routing.
///
/// # Lock ordering
///
/// To avoid deadlocks, acquire locks in this order and release them before
/// any `.await` that does not strictly need the lock:
///
/// 1. `config`
/// 2. `providers`
/// 3. `health`
/// 4. `usage_fetchers`
///
/// Write locks must never be held across an `.await`.
pub struct ModelRouter {
    /// Configuration
    config: RwLock<ModelRouterConfig>,
    /// Provider instances
    providers: RwLock<HashMap<String, Arc<dyn Provider + Send + Sync>>>,
    /// Health tracking per provider
    health: RwLock<HashMap<String, ProviderHealth>>,
    /// Active fallback chains
    fallback_chains: RwLock<HashMap<String, Vec<FallbackEntry>>>,
    /// Auth profile manager for API key rotation
    pub auth_profiles: AuthProfileManager,
    /// Per-provider usage tracker
    pub usage_tracker: ProviderUsageTracker,
    /// Dynamic model catalog with discovery and suppression
    pub model_catalog: ModelCatalog,
    /// Remote usage quota fetchers keyed by provider name.
    pub usage_fetchers: RwLock<UsageFetcherRegistry>,
    /// Optional SQLite pool for persisting auth profile state across restarts.
    db_pool: Option<sqlx::Pool<sqlx::Sqlite>>,
    /// Optional task registry for tracking the health-check background task.
    task_registry: Option<Arc<TaskRegistry>>,
    /// Shutdown token for cancelling the health-check background task.
    shutdown_token: CancellationToken,
    /// Pluggable task classifier for cost-aware routing.
    classifier: Box<dyn TaskClassifierImpl>,
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new(ModelRouterConfig::default())
    }
}

impl ModelRouter {
    /// Create a new model router
    pub fn new(config: ModelRouterConfig) -> Self {
        Self {
            config: RwLock::new(config),
            providers: RwLock::new(HashMap::new()),
            health: RwLock::new(HashMap::new()),
            fallback_chains: RwLock::new(HashMap::new()),
            auth_profiles: AuthProfileManager::new(),
            usage_tracker: ProviderUsageTracker::new(UsageTrackerConfig::default()),
            model_catalog: ModelCatalog::new(),
            usage_fetchers: RwLock::new(UsageFetcherRegistry::default()),
            db_pool: None,
            task_registry: None,
            shutdown_token: CancellationToken::new(),
            classifier: Box::new(KeywordTaskClassifier),
        }
    }

    /// Attach a SQLite connection pool for persisting auth profile state.
    pub fn with_db_pool(mut self, pool: sqlx::Pool<sqlx::Sqlite>) -> Self {
        self.db_pool = Some(pool);
        self
    }

    /// Attach the task registry used to track background health checks.
    pub fn with_task_registry(mut self, registry: Arc<TaskRegistry>) -> Self {
        self.task_registry = Some(registry);
        self
    }

    /// Attach the shutdown token used to cancel background health checks.
    pub fn with_shutdown_token(mut self, token: CancellationToken) -> Self {
        self.shutdown_token = token;
        self
    }

    /// Attach a custom task classifier for cost-aware routing.
    pub fn with_classifier(mut self, classifier: Box<dyn TaskClassifierImpl>) -> Self {
        self.classifier = classifier;
        self
    }

    /// Return a clone of the current router configuration.
    pub async fn router_config(&self) -> ModelRouterConfig {
        self.config.read().await.clone()
    }

    /// Return a clone of a single alias configuration, if it exists.
    pub async fn alias_config(&self, name: &str) -> Option<ModelAlias> {
        self.config.read().await.aliases.get(name).cloned()
    }

    /// Return all aliases along with their configurations.
    pub async fn aliases_with_configs(&self) -> Vec<(String, ModelAlias)> {
        self.config
            .read()
            .await
            .aliases
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    // ==================== INITIALIZATION ====================

    /// Initialize providers, fallback chains, and model catalog from config.
    pub async fn initialize(&self) -> crate::Result<()> {
        // Wire up persistent store if a database pool is available
        if let Some(ref pool) = self.db_pool {
            let store = std::sync::Arc::new(
                crate::model_router::auth_profile_store::AuthProfileStore::new(pool.clone()),
            );
            self.auth_profiles.set_store(store).await;
        }

        let config = self.config.read().await;

        for (name, provider_config) in &config.providers {
            info!("Initializing provider: {}", name);

            let auth_config = provider_config.derived_auth_profile_config();
            self.auth_profiles
                .register_from_config(name, &auth_config)
                .await;
            info!("Registered auth profile for '{}' with {} key(s)", name, auth_config.keys.len());

            let provider = self.create_provider(provider_config).await?;

            let mut providers = self.providers.write().await;
            providers.insert(name.clone(), provider);

            let mut health = self.health.write().await;
            health.insert(name.clone(), ProviderHealth::default());

            if matches!(provider_config.provider_type, ProviderType::OpenAi) {
                let api_key = provider_config.effective_key().await;
                if !api_key.is_empty() {
                    let fetcher = OpenAiUsageFetcher::new(api_key);
                    let mut fetchers = self.usage_fetchers.write().await;
                    fetchers.register(name.clone(), Arc::new(fetcher));
                }
            }
        }

        // Initialize fallback chains
        self.init_fallback_chains_from_config(&config).await;

        // Initialize model catalog from static aliases
        let alias_tuples: Vec<(String, String, String)> = config
            .aliases
            .values()
            .map(|a| (a.name.clone(), a.provider.clone(), a.model.clone()))
            .collect();
        drop(config);

        self.model_catalog
            .add_source(Box::new(model_catalog::StaticModelSource::new(alias_tuples)))
            .await;
        if let Err(e) = self.model_catalog.discover().await {
            warn!("Model catalog discovery failed: {}", e);
        }

        Ok(())
    }

    /// Initialize fallback chains and model catalog without re-creating
    /// providers.  Safe to call after `add_provider()` for production init.
    pub async fn init_catalog_and_chains(&self) {
        let config = self.config.read().await;
        self.init_fallback_chains_from_config(&config).await;

        let alias_tuples: Vec<(String, String, String)> = config
            .aliases
            .values()
            .map(|a| (a.name.clone(), a.provider.clone(), a.model.clone()))
            .collect();
        drop(config);

        self.model_catalog
            .add_source(Box::new(model_catalog::StaticModelSource::new(alias_tuples)))
            .await;
        if let Err(e) = self.model_catalog.discover().await {
            warn!("Model catalog discovery failed: {}", e);
        }
    }

    async fn init_fallback_chains_from_config(&self, config: &ModelRouterConfig) {
        let mut chains = self.fallback_chains.write().await;
        for (alias, provider_list) in &config.fallback_chains {
            let model = config.aliases.get(alias).map(|a| a.model.clone());
            if model.is_none() {
                warn!(
                    "Fallback chain references alias '{}' which is not defined in aliases — model \
                     will be empty",
                    alias
                );
            }
            let entries: Vec<FallbackEntry> = provider_list
                .iter()
                .map(|p| FallbackEntry {
                    provider: p.clone(),
                    model: model.clone().unwrap_or_default(),
                    enabled: true,
                    health_score: 100,
                })
                .collect();
            chains.insert(alias.clone(), entries);
        }
    }

    // ==================== HEALTH CHECKS ====================

    /// Start the health check background task
    pub fn start_health_checks(self: Arc<Self>) {
        let token = self.shutdown_token.clone();
        let registry = self.task_registry.clone();
        let handle = tokio::spawn(async move {
            let interval_secs = {
                let config = self.config.read().await;
                config.health_check_interval_secs
            };
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        info!("Model router health checks received shutdown signal, exiting");
                        break;
                    }
                    _ = interval.tick() => {}
                }
                self.run_health_checks().await;
            }
        });

        if let Some(registry) = registry {
            tokio::spawn(async move {
                registry
                    .insert_join("model_router:health_check", handle)
                    .await;
            });
        }
    }

    /// Run periodic health checks
    async fn run_health_checks(&self) {
        let provider_names: Vec<String> = {
            let providers = self.providers.read().await;
            providers.keys().cloned().collect()
        };
        let reset_secs = {
            let config = self.config.read().await;
            config.circuit_breaker_reset_secs
        };

        for name in provider_names {
            // First handle circuit-breaker state transitions (Open → HalfOpen).
            {
                let mut health = self.health.write().await;
                if let Some(h) = health.get_mut(&name) {
                    h.last_health_check = Some(chrono::Utc::now());

                    if h.state == CircuitState::Open {
                        if let Some(last_failure) = h.last_failure {
                            let elapsed = chrono::Utc::now() - last_failure;
                            if elapsed.num_seconds() >= reset_secs as i64 {
                                info!("Circuit breaker half-open for provider: {}", name);
                                h.state = CircuitState::HalfOpen;
                            }
                        }
                    }
                }
            }

            let provider = {
                let providers = self.providers.read().await;
                providers.get(&name).cloned()
            };

            if let Some(provider) = provider {
                let start = std::time::Instant::now();
                match provider.health_check().await {
                    Ok(true) => self.record_success(&name, start.elapsed()).await,
                    Ok(false) => {
                        debug!("Health probe reported unhealthy for {}", name);
                        self.record_failure(&name, None).await;
                    }
                    Err(e) => {
                        debug!("Health probe failed for {}: {}", name, e);
                        self.record_failure(&name, None).await;
                    }
                }
            }
        }
    }

    /// Create a provider instance from config
    async fn create_provider(
        &self,
        config: &ProviderConfig,
    ) -> crate::Result<Arc<dyn Provider + Send + Sync>> {
        let api_key = config.effective_key().await;
        let provider_type = config.provider_type.to_string();

        // Map legacy provider_type names to preset names
        let provider_type = match provider_type.as_str() {
            "moonshot" => "kimi",
            other => other,
        };

        use crate::providers::resolver::resolve_from_config;

        resolve_from_config(
            provider_type,
            Some(api_key),
            None, // protocol: auto-detect from preset default
            config.base_url.clone(),
            None, // model: use preset default
            None, // max_context: use preset default
            None, // supports_vision: use preset default
            None, // supports_tools: use preset default
            None, // stream_family: use preset default
            None, // auth_method: use preset default
        )
        .map(|p| p as Arc<dyn Provider + Send + Sync>)
    }

    /// Rotate the active credential for a provider in place.
    async fn rotate_provider_credential(
        &self,
        provider_name: &str,
        cooldown_secs: u64,
    ) -> crate::Result<()> {
        let provider = {
            let providers = self.providers.read().await;
            providers.get(provider_name).cloned().ok_or_else(|| {
                crate::error::ConfigError::InvalidValue {
                    key: "provider".to_string(),
                    message: format!("Unknown provider: {provider_name}"),
                }
            })?
        };

        match self
            .auth_profiles
            .rotate(provider_name, cooldown_secs)
            .await
        {
            Some(new_key) => {
                provider
                    .set_credential(Credential::api_key(new_key))
                    .await?;
                info!("Rotated credential for provider '{provider_name}'");
                Ok(())
            }
            None => Err(crate::error::SyscityError::ExternalService {
                source: format!(
                    "No available API keys for provider '{provider_name}' after rotation"
                ),
                cause: None,
            }),
        }
    }

    /// Compute the effective cooldown for a failure class.
    async fn cooldown_for_failure(&self, provider: &str, class: FailureClass) -> u64 {
        let config = self.config.read().await;
        let base = config
            .providers
            .get(provider)
            .map(|pc| pc.derived_auth_profile_config().cooldown_secs)
            .unwrap_or(60);
        drop(config);
        class.default_backoff_secs().max(base)
    }

    // ==================== RECORDING SUCCESS / FAILURE ====================

    /// Record a successful completion, updating health, auth profile and usage.
    async fn record_completion_success(
        &self,
        provider: &str,
        latency: Duration,
        usage: Option<Usage>,
        model: &str,
    ) {
        self.record_success(provider, latency).await;
        self.auth_profiles.record_success(provider).await;
        if let Some(usage) = usage {
            self.usage_tracker.record(provider, usage, model).await;
        }
    }

    /// Record a successful request
    async fn record_success(&self, provider: &str, latency: Duration) {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(provider) {
            h.successes += 1;
            h.failures = 0;
            h.state = CircuitState::Closed;

            let latency_ms = latency.as_millis() as u64;
            h.avg_latency_ms = (h.avg_latency_ms * 9 + latency_ms) / 10;
        }
    }

    /// Record a failed request with optional failure classification.
    ///
    /// Uses the classification to make smarter circuit-breaker decisions
    /// (e.g. rate-limit errors open the circuit faster).
    async fn record_failure(&self, provider: &str, class: Option<FailureClass>) {
        let config = self.config.read().await;
        let threshold = config.circuit_breaker_threshold;
        drop(config);

        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(provider) {
            h.failures += 1;
            h.last_failure = Some(chrono::Utc::now());

            let effective_threshold = match class {
                Some(FailureClass::RateLimit) => threshold.saturating_sub(2).max(1),
                Some(FailureClass::Overloaded) => threshold.saturating_sub(1).max(1),
                _ => threshold,
            };

            // Fix (Issue 3): also transition HalfOpen → Open on failure.
            if h.failures >= effective_threshold && h.state != CircuitState::Open {
                warn!(
                    "Circuit breaker opened for provider: {provider} ({} failures, class={:?})",
                    h.failures, class
                );
                h.state = CircuitState::Open;
            }
        }
    }

    /// Handle a provider failure, applying key rotation/disable and a single
    /// retry when appropriate.
    async fn handle_provider_failure<T, F, Fut>(
        &self,
        provider_name: &str,
        model: &str,
        class: FailureClass,
        error: &crate::error::SyscityError,
        retry_once: F,
    ) -> crate::Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = crate::Result<T>>,
    {
        if class == FailureClass::ModelNotFound {
            self.model_catalog.suppress(provider_name, model).await;
            warn!("Auto-suppressed model {provider_name}:{model}");
        }

        if class.should_disable_key() {
            let cooldown = self.cooldown_for_failure(provider_name, class).await;
            if let Err(disable_err) = self
                .rotate_provider_credential(provider_name, cooldown)
                .await
            {
                error!("Key disable/rotation failed for provider {provider_name}: {disable_err}");
            }
            self.record_failure(provider_name, Some(class)).await;
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Provider {provider_name} auth disabled: {error}"),
                cause: None,
            });
        }

        if class.should_rotate_key() {
            let cooldown = self.cooldown_for_failure(provider_name, class).await;
            match self
                .rotate_provider_credential(provider_name, cooldown)
                .await
            {
                Ok(()) => match retry_once().await {
                    Ok(response) => return Ok(response),
                    Err(e2) => {
                        let class2 = FailureClass::from_error(&e2, None);
                        error!("Provider {provider_name} failed after key rotation: {e2}");
                        self.record_failure(provider_name, Some(class2)).await;
                        return Err(e2);
                    }
                },
                Err(rotate_err) => {
                    error!("Key rotation failed for provider {provider_name}: {rotate_err}");
                    self.record_failure(provider_name, Some(class)).await;
                    return Err(rotate_err);
                }
            }
        }

        self.record_failure(provider_name, Some(class)).await;
        Err(crate::error::SyscityError::ExternalService {
            source: format!("Provider {provider_name} failed: {error}"),
            cause: None,
        })
    }

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

    // ==================== COST-AWARE ROUTING ====================

    /// Complete a request with cost-aware automatic model selection
    pub async fn complete_auto(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionResponse> {
        let config = self.config.read().await;

        let alias_name = if let Some(ref cost_aware) = config.cost_aware {
            if cost_aware.enabled {
                drop(config);
                return self.complete_with_cost_routing(messages, tools).await;
            }
            cost_aware.default_alias.clone()
        } else {
            config.default_model.clone()
        };
        drop(config);

        self.complete(&alias_name, messages, tools).await
    }

    /// Internal: route based on task classification and cost
    async fn complete_with_cost_routing(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionResponse> {
        let task_type = self.classifier.classify(&messages);
        info!("Task classified as: {:?}", task_type);

        let config = self.config.read().await;
        let Some(cost_aware) = config.cost_aware.as_ref() else {
            let default = config.default_model.clone();
            drop(config);
            return self.complete(&default, messages, tools).await;
        };

        // Check budget limit
        if let Some(cheapest) = Self::cheapest_model_on_budget_exceeded(cost_aware) {
            drop(config);
            return self.complete(&cheapest, messages, tools).await;
        }

        // Resolve alias from routing rules
        let alias_name = Self::resolve_alias_for_task(cost_aware, &task_type, &messages);
        drop(config);

        // Complete and track cost
        let alias_name_for_cost = alias_name.clone();
        let response = self.complete(&alias_name, messages, tools).await?;

        // Track cost: config is lock #1 (per doc at line 48-58) and we hold
        // no other locks at this point, so acquiring config.write() is safe.
        if let Some(ref usage) = response.usage {
            let mut config = self.config.write().await;
            if let Some(ref mut cost_aware) = config.cost_aware {
                if let Some(cost) = cost_aware.model_costs.get(&alias_name_for_cost) {
                    let estimated = cost.estimate(usage);
                    cost_aware.daily_spend_usd += estimated;
                    info!(
                        "Cost tracked: ${estimated:.4} for '{alias_name_for_cost}' (task: \
                         {task_type:?})"
                    );
                }
            }
        }

        Ok(response)
    }

    /// Get cheapest model alias when budget is exceeded, or `None` if within
    /// budget.
    fn cheapest_model_on_budget_exceeded(cost_aware: &CostAwareConfig) -> Option<String> {
        let budget = cost_aware.budget_limit_usd?;
        let current_spend = cost_aware.daily_spend_usd;
        if current_spend < budget {
            return None;
        }
        warn!(
            "Daily budget exceeded: ${:.2} / ${:.2}. Falling back to cheapest model.",
            current_spend, budget
        );
        let cheapest = cost_aware
            .model_costs
            .iter()
            .min_by(|a, b| {
                let a_total = a.1.input_cost_per_1k + a.1.output_cost_per_1k;
                let b_total = b.1.input_cost_per_1k + b.1.output_cost_per_1k;
                a_total
                    .partial_cmp(&b_total)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| cost_aware.default_alias.clone());
        Some(cheapest)
    }

    /// Resolve the model alias to use for a given task type based on routing
    /// rules.
    fn resolve_alias_for_task(
        cost_aware: &CostAwareConfig,
        task_type: &TaskType,
        messages: &[Message],
    ) -> String {
        let rule = cost_aware
            .routing_rules
            .iter()
            .find(|r| r.task_type == *task_type)
            .or_else(|| {
                cost_aware
                    .routing_rules
                    .iter()
                    .find(|r| r.task_type == TaskType::Unknown)
            });

        let Some(rule) = rule else {
            return cost_aware.default_alias.clone();
        };

        let estimated_tokens: u32 = messages.iter().map(|m| m.content.len() as u32 / 4).sum();
        if let Some(max_tokens) = rule.max_input_tokens {
            if estimated_tokens > max_tokens {
                info!(
                    "Estimated tokens ({estimated_tokens}) exceeds max for '{}' ({max_tokens}), \
                     using fallback",
                    rule.preferred_alias
                );
                return rule
                    .fallback_alias
                    .clone()
                    .unwrap_or_else(|| rule.preferred_alias.clone());
            }
        }
        rule.preferred_alias.clone()
    }

    /// Get current daily spend
    pub async fn get_daily_spend(&self) -> f64 {
        let config = self.config.read().await;
        config
            .cost_aware
            .as_ref()
            .map(|c| c.daily_spend_usd)
            .unwrap_or(0.0)
    }

    /// Reset daily spend counter
    pub async fn reset_daily_spend(&self) {
        let mut config = self.config.write().await;
        if let Some(ref mut cost_aware) = config.cost_aware {
            cost_aware.daily_spend_usd = 0.0;
            info!("Daily spend counter reset");
        }
    }

    // ==================== USAGE SNAPSHOTS WITH QUOTA ====================

    /// Get a usage snapshot enriched with remote quota.
    pub async fn snapshot_with_quota(&self, provider: &str) -> Option<ProviderUsageSnapshot> {
        let mut snapshot = self.usage_tracker.snapshot(provider).await?;

        let fetchers = self.usage_fetchers.read().await;
        if let Some(fetcher) = fetchers.get(provider) {
            match fetcher.fetch().await {
                Ok(Some(quota)) => {
                    snapshot.quota = Some(quota);
                }
                Ok(None) => {
                    drop(fetchers);
                    snapshot.quota = self.local_budget_quota(provider).await;
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch usage quota for {}: {}; falling back to local budget",
                        provider, e
                    );
                    drop(fetchers);
                    snapshot.quota = self.local_budget_quota(provider).await;
                }
            }
        } else {
            drop(fetchers);
            snapshot.quota = self.local_budget_quota(provider).await;
        }

        snapshot.last_updated = Utc::now();
        Some(snapshot)
    }

    /// Get all usage snapshots enriched with remote quota.
    pub async fn all_snapshots_with_quota(&self) -> Vec<ProviderUsageSnapshot> {
        let base_snapshots = self.usage_tracker.all_snapshots().await;

        let fetchers = self.usage_fetchers.read().await;
        let futures = base_snapshots
            .into_iter()
            .map(|snapshot| {
                let provider = snapshot.provider.clone();
                let fetcher = fetchers.get(&provider).clone();
                async move {
                    let provider = snapshot.provider.clone();
                    let quota = if let Some(fetcher) = fetcher {
                        match fetcher.fetch().await {
                            Ok(Some(q)) => Some(q),
                            Ok(None) => None,
                            Err(e) => {
                                warn!("Failed to fetch usage quota for {}: {}", provider, e);
                                None
                            }
                        }
                    } else {
                        None
                    };
                    (snapshot, quota)
                }
            })
            .collect::<Vec<_>>();
        drop(fetchers);

        let results = join_all(futures).await;
        let mut enriched = Vec::with_capacity(results.len());
        for (mut snapshot, remote_quota) in results {
            let provider = snapshot.provider.clone();
            snapshot.quota = if let Some(q) = remote_quota {
                Some(q)
            } else {
                self.local_budget_quota(&provider).await
            };
            snapshot.last_updated = Utc::now();
            enriched.push(snapshot);
        }

        enriched
    }

    async fn local_budget_quota(&self, provider: &str) -> Option<UsageQuota> {
        let snapshot = self.usage_tracker.snapshot(provider).await?;
        let config = self.usage_tracker.config();

        let today_cost: f64 = snapshot
            .windows
            .iter()
            .filter(|w| w.label == "today")
            .map(|w| w.estimated_cost_usd)
            .sum();

        let month_cost: f64 = snapshot
            .windows
            .iter()
            .filter(|w| w.label == "this_month")
            .map(|w| w.estimated_cost_usd)
            .sum();

        let fetcher = LocalBudgetFetcher::new(
            provider,
            config.daily_budget_usd,
            config.monthly_budget_usd,
            today_cost,
            month_cost,
        );
        fetcher.fetch().await.ok().flatten()
    }

    /// Resolve an alias, upgrading to a capability-compatible model if needed.
    async fn resolve_alias_with_capabilities(
        &self,
        alias: &ModelAlias,
        request: &CompletionRequest,
    ) -> ModelAlias {
        if !request.requires_vision && !request.requires_tools && !request.requires_reasoning {
            return alias.clone();
        }

        let entry = self.model_catalog.get(&alias.provider, &alias.model).await;

        let compatible = entry.is_some_and(|e| {
            (!request.requires_vision || e.supports_vision)
                && (!request.requires_tools || e.supports_tools)
                && (!request.requires_reasoning || e.supports_reasoning)
        });

        if compatible {
            return alias.clone();
        }

        // Search catalog for cheapest compatible model
        let candidates = self.model_catalog.list().await;
        let mut best: Option<(&ModelCatalogEntry, f64)> = None;

        for c in &candidates {
            if (!request.requires_vision || c.supports_vision)
                && (!request.requires_tools || c.supports_tools)
                && (!request.requires_reasoning || c.supports_reasoning)
            {
                let cost = c
                    .pricing
                    .as_ref()
                    .map(|p| p.input_per_1k + p.output_per_1k)
                    .unwrap_or(f64::MAX);
                if best.is_none_or(|(_, best_cost)| cost < best_cost) {
                    best = Some((c, cost));
                }
            }
        }

        if let Some((entry, _)) = best {
            info!(
                "Capability routing: upgraded '{}' (provider={}, model={}) to '{}' (provider={}, \
                 model={}) for vision={} tools={} reasoning={}",
                alias.name,
                alias.provider,
                alias.model,
                entry.name,
                entry.provider,
                entry.id,
                request.requires_vision,
                request.requires_tools,
                request.requires_reasoning,
            );
            let mut upgraded = alias.clone();
            upgraded.provider = entry.provider.clone();
            upgraded.model = entry.id.clone();
            return upgraded;
        }

        alias.clone()
    }

    // ==================== PROVIDER CHAIN ====================

    /// Get the ordered list of providers to try
    async fn get_provider_chain(&self, alias: &ModelAlias) -> Vec<FallbackEntry> {
        let chains = self.fallback_chains.read().await;

        if let Some(chain) = chains.get(&alias.name) {
            return chain.clone();
        }

        vec![FallbackEntry {
            provider: alias.provider.clone(),
            model: alias.model.clone(),
            enabled: true,
            health_score: 100,
        }]
    }

    /// Check if circuit breaker is open for a provider.
    ///
    /// This is an **optimistic** check — a concurrent health-check task may
    /// transition the state from `Open → HalfOpen` between when this returns
    /// `false` and when the actual provider call completes.  The downstream
    /// [`route_with_fallback`] handles that safely by recording the failure
    /// and re-opening the circuit (see [`record_failure`]).
    async fn is_circuit_open(&self, provider: &str) -> bool {
        let health = self.health.read().await;
        health
            .get(provider)
            .is_some_and(|h| h.state == CircuitState::Open)
    }

    // ==================== HEALTH STATUS ====================

    /// Get health status for all providers
    pub async fn get_health_status(&self) -> HashMap<String, ProviderHealth> {
        let health = self.health.read().await;
        health
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    ProviderHealth {
                        state: v.state,
                        failures: v.failures,
                        successes: v.successes,
                        last_failure: v.last_failure,
                        avg_latency_ms: v.avg_latency_ms,
                        last_health_check: v.last_health_check,
                    },
                )
            })
            .collect()
    }

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

    // ==================== AUTH PROFILE MANAGEMENT ====================

    /// Get auth profile status for a provider
    pub async fn get_auth_profile_status(&self, provider_name: &str) -> Option<ProfileStatus> {
        self.auth_profiles.get_status(provider_name).await
    }

    /// Get auth profile status for all providers
    pub async fn list_auth_profiles(&self) -> Vec<ProfileStatus> {
        self.auth_profiles.all_statuses().await
    }

    /// Manually rotate the auth key for a provider
    pub async fn rotate_auth_key(&self, provider_name: &str) -> crate::Result<String> {
        let provider = {
            let providers = self.providers.read().await;
            providers.get(provider_name).cloned().ok_or_else(|| {
                crate::error::ConfigError::InvalidValue {
                    key: "provider".to_string(),
                    message: format!("Unknown provider: {provider_name}"),
                }
            })?
        };

        match self.auth_profiles.rotate(provider_name, 60).await {
            Some(new_key) => {
                provider
                    .set_credential(Credential::api_key(new_key.clone()))
                    .await?;
                info!("Manually rotated auth key for provider '{provider_name}'");
                Ok(new_key)
            }
            None => Err(crate::error::SyscityError::ExternalService {
                source: format!(
                    "No available API keys for provider '{provider_name}' after rotation"
                ),
                cause: None,
            }),
        }
    }
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::gateway::task_registry::TaskRegistry;
    use crate::model_router::auth_profile::{AuthKeyConfig, AuthProfileConfig};
    use crate::model_router::config::{
        CircuitState, CostAwareConfig, ModelAlias, ModelRouterConfig, ProviderConfig, ProviderType,
    };
    use crate::model_router::usage_fetcher::UsageFetcher;
    use crate::model_router::usage_tracker::{QuotaSource, UsageQuota};
    use crate::providers::{
        CompletionRequest, CompletionResponse, CompletionStream, Message, Provider, Usage,
    };

    #[tokio::test]
    async fn default_config_has_no_aliases() {
        let config = ModelRouterConfig::default();
        assert!(config.aliases.is_empty());
        assert_eq!(config.default_model, "");
    }

    #[tokio::test]
    async fn aliases_with_configs_returns_all_aliases() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "fast".to_string(),
                provider: "openai".to_string(),
                model: "gpt-3.5".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_alias(ModelAlias {
                name: "smart".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let aliases = router.aliases_with_configs().await;
        assert_eq!(aliases.len(), 2);
        assert!(aliases.iter().any(|(n, _)| n == "fast"));
        assert!(aliases.iter().any(|(n, _)| n == "smart"));
    }

    #[tokio::test]
    async fn alias_config_returns_single_alias() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let alias = router.alias_config("default").await;
        assert!(alias.is_some());
        assert_eq!(alias.unwrap().provider, "anthropic");
        assert!(router.alias_config("missing").await.is_none());
    }

    #[tokio::test]
    async fn router_config_returns_clone() {
        let mut config = ModelRouterConfig::default();
        config.default_model = "default".to_string();
        let router = ModelRouter::new(config);

        let snapshot = router.router_config().await;
        assert_eq!(snapshot.default_model, "default");
    }

    #[tokio::test]
    async fn list_aliases_returns_empty_by_default() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let aliases = router.list_aliases().await;
        assert!(aliases.is_empty());
    }

    #[tokio::test]
    async fn set_alias_adds_new_alias() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let alias = ModelAlias {
            name: "coding".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            temperature: Some(0.2),
            max_tokens: Some(4096),
        };
        router.set_alias(alias.clone()).await;

        let aliases = router.list_aliases().await;
        assert_eq!(aliases.len(), 1);
        assert!(aliases.contains(&"coding".to_string()));
    }

    #[tokio::test]
    async fn remove_alias_deletes_alias() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "fast".to_string(),
                provider: "openai".to_string(),
                model: "gpt-3.5".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let removed = router.remove_alias("fast").await;
        assert!(removed);

        let aliases = router.list_aliases().await;
        assert!(aliases.is_empty());
    }

    #[tokio::test]
    async fn remove_unknown_alias_returns_false() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let removed = router.remove_alias("nonexistent").await;
        assert!(!removed);
    }

    #[tokio::test]
    async fn switch_default_model_changes_default() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_alias(ModelAlias {
                name: "fast".to_string(),
                provider: "openai".to_string(),
                model: "gpt-3.5".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router.switch_default_model("fast").await.unwrap();

        let default = router.get_default_model().await;
        assert_eq!(default, "fast");
    }

    #[tokio::test]
    async fn switch_unknown_model_fails() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let result = router.switch_default_model("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_providers_returns_empty_when_none() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let providers = router.list_providers().await;
        assert_eq!(providers.len(), 0);
    }

    #[tokio::test]
    async fn add_and_remove_provider() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let config = ProviderConfig {
            provider_type: ProviderType::Anthropic,
            api_key: "test-key".to_string().into(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        router.add_provider("test-provider", config).await.unwrap();
        let providers = router.list_providers().await;
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "test-provider");

        router.remove_provider("test-provider").await.unwrap();
        let providers = router.list_providers().await;
        assert_eq!(providers.len(), 0);
    }

    #[tokio::test]
    async fn enable_disable_provider() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let config = ProviderConfig {
            provider_type: ProviderType::Anthropic,
            api_key: "test-key".to_string().into(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        router.add_provider("p1", config).await.unwrap();

        let providers = router.list_providers().await;
        assert!(providers[0].enabled);
        assert_eq!(providers[0].circuit_state, CircuitState::Closed);

        router.disable_provider("p1").await.unwrap();
        let providers = router.list_providers().await;
        assert!(!providers[0].enabled);
        assert_eq!(providers[0].circuit_state, CircuitState::Open);

        router.enable_provider("p1").await.unwrap();
        let providers = router.list_providers().await;
        assert!(providers[0].enabled);
        assert_eq!(providers[0].circuit_state, CircuitState::Closed);
    }

    #[tokio::test]
    async fn enable_unknown_provider_fails() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let result = router.enable_provider("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fallback_chain_roundtrip() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_fallback_chain("default", vec!["p1".to_string(), "p2".to_string()])
            .await
            .unwrap();

        let chain = router.get_fallback_chain("default").await;
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0], "p1");
        assert_eq!(chain[1], "p2");
    }

    #[tokio::test]
    async fn fallback_chain_for_unknown_alias_returns_empty() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let chain = router.get_fallback_chain("nonexistent").await;
        assert!(chain.is_empty());
    }

    #[tokio::test]
    async fn get_provider_health_returns_info() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let config = ProviderConfig {
            provider_type: ProviderType::Anthropic,
            api_key: "test-key".to_string().into(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        router.add_provider("p1", config).await.unwrap();

        let health = router.get_provider_health("p1").await;
        assert!(health.is_some());
        let info = health.unwrap();
        assert_eq!(info.state, "Closed");
        assert_eq!(info.failures, 0);
        assert_eq!(info.successes, 0);
        assert_eq!(info.avg_latency_ms, 0);
    }

    #[tokio::test]
    async fn get_provider_health_unknown_returns_none() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let health = router.get_provider_health("nonexistent").await;
        assert!(health.is_none());
    }

    #[tokio::test]
    async fn check_provider_health_unknown_fails() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let result = router.check_provider_health("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn provider_list_includes_circuit_state() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let config = ProviderConfig {
            provider_type: ProviderType::OpenAi,
            api_key: "key".to_string().into(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        router.add_provider("p1", config).await.unwrap();
        let providers = router.list_providers().await;
        assert_eq!(providers[0].circuit_state, CircuitState::Closed);

        router.disable_provider("p1").await.unwrap();
        let providers = router.list_providers().await;
        assert_eq!(providers[0].circuit_state, CircuitState::Open);
    }

    #[tokio::test]
    async fn fallback_chain_set_and_clear() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        router
            .set_fallback_chain("default", vec!["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        let chain = router.get_fallback_chain("default").await;
        assert_eq!(chain, vec!["a", "b"]);

        router.set_fallback_chain("default", vec![]).await.unwrap();
        let chain = router.get_fallback_chain("default").await;
        assert!(chain.is_empty());
    }

    #[tokio::test]
    async fn set_fallback_chain_unknown_alias_fails() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let result = router
            .set_fallback_chain("nonexistent", vec!["a".to_string()])
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn switch_default_model_persists() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_alias(ModelAlias {
                name: "fast".to_string(),
                provider: "openai".to_string(),
                model: "gpt-3.5".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_alias(ModelAlias {
                name: "smart".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3-opus".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        router.switch_default_model("fast").await.unwrap();
        assert_eq!(router.get_default_model().await, "fast");

        router.switch_default_model("smart").await.unwrap();
        assert_eq!(router.get_default_model().await, "smart");

        router.switch_default_model("default").await.unwrap();
        assert_eq!(router.get_default_model().await, "default");
    }

    #[tokio::test]
    async fn default_model_is_empty_by_default() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let default = router.get_default_model().await;
        assert_eq!(default, "");
    }

    #[tokio::test]
    async fn cost_aware_disabled_by_default() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let spend = router.get_daily_spend().await;
        assert_eq!(spend, 0.0);
    }

    #[tokio::test]
    async fn cost_aware_config_default_routing_rules() {
        let config = CostAwareConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.model_costs.len(), 3);
        assert_eq!(config.routing_rules.len(), 9);
        assert_eq!(config.default_alias, "default");
    }

    #[tokio::test]
    async fn reset_daily_spend_no_op_when_disabled() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router.reset_daily_spend().await;
        assert_eq!(router.get_daily_spend().await, 0.0);
    }

    #[tokio::test]
    async fn cost_aware_config_with_enabled_spend_tracking() {
        let mut config = ModelRouterConfig::default();
        let mut cost_aware = CostAwareConfig::default();
        cost_aware.enabled = true;
        cost_aware.daily_spend_usd = 5.0;
        config.cost_aware = Some(cost_aware);

        let router = ModelRouter::new(config);
        assert_eq!(router.get_daily_spend().await, 5.0);

        router.reset_daily_spend().await;
        assert_eq!(router.get_daily_spend().await, 0.0);
    }

    #[tokio::test]
    async fn health_check_uses_health_check_not_complete() {
        #[derive(Clone)]
        struct TrackingProvider {
            complete_count: Arc<Mutex<usize>>,
            health_check_count: Arc<Mutex<usize>>,
        }

        impl TrackingProvider {
            fn new() -> Self {
                Self {
                    complete_count: Arc::new(Mutex::new(0)),
                    health_check_count: Arc::new(Mutex::new(0)),
                }
            }
        }

        #[async_trait]
        impl Provider for TrackingProvider {
            fn name(&self) -> &str {
                "tracking"
            }
            fn default_model(&self) -> &str {
                "tracking-model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                *self.complete_count.lock().unwrap() += 1;
                Ok(CompletionResponse {
                    message: Message::assistant("ok"),
                    model: "tracking-model".to_string(),
                    usage: Some(Usage::default()),
                    finish_reason: Some("stop".to_string()),
                })
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                *self.health_check_count.lock().unwrap() += 1;
                Ok(true)
            }
            async fn set_credential(
                &self,
                _credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        let mut config = ModelRouterConfig::default();
        config.health_check_interval_secs = 1;
        let registry = Arc::new(TaskRegistry::new());
        let token = CancellationToken::new();
        let router = Arc::new(
            ModelRouter::new(config)
                .with_task_registry(registry.clone())
                .with_shutdown_token(token.clone()),
        );

        let provider = TrackingProvider::new();
        router
            .add_provider_instance("tracking", Arc::new(provider.clone()))
            .await
            .unwrap();

        router.clone().start_health_checks();
        tokio::time::sleep(Duration::from_millis(2500)).await;
        token.cancel();

        assert!(*provider.health_check_count.lock().unwrap() > 0);
        assert_eq!(*provider.complete_count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn health_check_task_respects_shutdown() {
        #[derive(Clone)]
        struct HealthyProvider;

        #[async_trait]
        impl Provider for HealthyProvider {
            fn name(&self) -> &str {
                "healthy"
            }
            fn default_model(&self) -> &str {
                "healthy-model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                Ok(CompletionResponse {
                    message: Message::assistant("ok"),
                    model: "healthy-model".to_string(),
                    usage: Some(Usage::default()),
                    finish_reason: Some("stop".to_string()),
                })
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                Ok(true)
            }
            async fn set_credential(
                &self,
                _credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        let mut config = ModelRouterConfig::default();
        config.health_check_interval_secs = 60;
        let registry = Arc::new(TaskRegistry::new());
        let token = CancellationToken::new();
        let router = Arc::new(
            ModelRouter::new(config)
                .with_task_registry(registry.clone())
                .with_shutdown_token(token.clone()),
        );

        router
            .add_provider_instance("healthy", Arc::new(HealthyProvider))
            .await
            .unwrap();

        router.clone().start_health_checks();

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(registry.contains("model_router:health_check").await);

        token.cancel();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let handle = registry
            .remove_join_or_abort("model_router:health_check")
            .await;
        assert!(handle.is_some());
        let result = tokio::time::timeout(Duration::from_secs(2), handle.unwrap()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn auth_failure_rotates_key_and_retries_successfully() {
        #[derive(Clone)]
        struct RotatingProvider {
            calls: Arc<Mutex<usize>>,
            rotated_key: Arc<Mutex<Option<String>>>,
        }

        impl RotatingProvider {
            fn new() -> Self {
                Self {
                    calls: Arc::new(Mutex::new(0)),
                    rotated_key: Arc::new(Mutex::new(None)),
                }
            }
        }

        #[async_trait]
        impl Provider for RotatingProvider {
            fn name(&self) -> &str {
                "rotator"
            }
            fn default_model(&self) -> &str {
                "rotator-model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                let mut calls = self.calls.lock().unwrap();
                *calls += 1;
                if *calls == 1 {
                    Err(crate::error::SyscityError::ExternalService {
                        source: "OpenAI API error 401: Unauthorized".into(),
                        cause: None,
                    })
                } else {
                    Ok(CompletionResponse {
                        message: Message::assistant("ok"),
                        model: "rotator-model".to_string(),
                        usage: Some(Usage::default()),
                        finish_reason: Some("stop".to_string()),
                    })
                }
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                Ok(true)
            }
            async fn set_credential(
                &self,
                credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                if let crate::model_router::Credential::ApiKey { key } = credential {
                    *self.rotated_key.lock().unwrap() = Some(key);
                }
                Ok(())
            }
        }

        let router = ModelRouter::new(ModelRouterConfig::default());
        let provider = Arc::new(RotatingProvider::new());
        router
            .add_provider_instance("rotator", provider.clone())
            .await
            .unwrap();

        let auth_config = AuthProfileConfig {
            keys: vec![
                AuthKeyConfig {
                    key: "first-key".to_string(),
                    label: "first".to_string(),
                },
                AuthKeyConfig {
                    key: "second-key".to_string(),
                    label: "second".to_string(),
                },
            ],
            cooldown_secs: 60,
            max_failures: 3,
        };
        router
            .auth_profiles
            .register_from_config("rotator", &auth_config)
            .await;

        router
            .set_alias(ModelAlias {
                name: "test".to_string(),
                provider: "rotator".to_string(),
                model: "rotator-model".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let response = router
            .complete("test", vec![Message::user("hi")], None)
            .await
            .expect("complete should succeed after key rotation");
        assert_eq!(response.message.content, "ok");
        assert_eq!(*provider.calls.lock().unwrap(), 2);
        assert_eq!(provider.rotated_key.lock().unwrap().as_deref(), Some("second-key"));
    }

    #[tokio::test]
    async fn content_policy_failure_is_not_retried() {
        #[derive(Clone)]
        struct ContentPolicyProvider {
            calls: Arc<Mutex<usize>>,
        }

        #[async_trait]
        impl Provider for ContentPolicyProvider {
            fn name(&self) -> &str {
                "content-policy"
            }
            fn default_model(&self) -> &str {
                "model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                *self.calls.lock().unwrap() += 1;
                Err(crate::error::SyscityError::ExternalService {
                    source: "OpenAI API error 400: Content policy violation".into(),
                    cause: None,
                })
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                Ok(true)
            }
            async fn set_credential(
                &self,
                _credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        let router = ModelRouter::new(ModelRouterConfig::default());
        let provider = Arc::new(ContentPolicyProvider { calls: Arc::new(Mutex::new(0)) });
        router
            .add_provider_instance("content-policy", provider.clone())
            .await
            .unwrap();

        router
            .set_alias(ModelAlias {
                name: "test".to_string(),
                provider: "content-policy".to_string(),
                model: "model".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let result = router
            .complete("test", vec![Message::user("hi")], None)
            .await;
        assert!(result.is_err());
        assert_eq!(*provider.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn concurrent_complete_calls_do_not_deadlock() {
        #[derive(Clone)]
        struct SlowProvider {
            calls: Arc<Mutex<usize>>,
        }

        #[async_trait]
        impl Provider for SlowProvider {
            fn name(&self) -> &str {
                "slow"
            }
            fn default_model(&self) -> &str {
                "slow-model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                tokio::time::sleep(Duration::from_millis(50)).await;
                *self.calls.lock().unwrap() += 1;
                Ok(CompletionResponse {
                    message: Message::assistant("ok"),
                    model: "slow-model".to_string(),
                    usage: Some(Usage::default()),
                    finish_reason: Some("stop".to_string()),
                })
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                Ok(true)
            }
            async fn set_credential(
                &self,
                _credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        let router = Arc::new(ModelRouter::new(ModelRouterConfig::default()));
        let provider = Arc::new(SlowProvider { calls: Arc::new(Mutex::new(0)) });
        router
            .add_provider_instance("slow", provider.clone())
            .await
            .unwrap();
        router
            .set_alias(ModelAlias {
                name: "test".to_string(),
                provider: "slow".to_string(),
                model: "slow-model".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let r1 = router.clone();
        let r2 = router.clone();
        let (first, second) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(2), async move {
                r1.complete("test", vec![Message::user("a")], None).await
            }),
            tokio::time::timeout(Duration::from_secs(2), async move {
                r2.complete("test", vec![Message::user("b")], None).await
            }),
        );

        assert!(first.is_ok(), "first complete timed out (possible deadlock)");
        assert!(second.is_ok(), "second complete timed out (possible deadlock)");
        assert!(first.unwrap().is_ok());
        assert!(second.unwrap().is_ok());
        assert_eq!(*provider.calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn all_snapshots_with_quota_fetches_for_all_providers() {
        #[derive(Clone)]
        struct MockUsageFetcher {
            remaining: f64,
        }

        #[async_trait]
        impl UsageFetcher for MockUsageFetcher {
            fn provider(&self) -> &str {
                "mock"
            }

            async fn fetch(&self) -> crate::Result<Option<UsageQuota>> {
                Ok(Some(UsageQuota {
                    remaining: self.remaining,
                    limit: 100.0,
                    reset_at: None,
                    unit: "usd".to_string(),
                    source: QuotaSource::Remote,
                }))
            }
        }

        let router = ModelRouter::new(ModelRouterConfig::default());
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        router.usage_tracker.record("openai", usage, "gpt-4o").await;
        router
            .usage_tracker
            .record("anthropic", usage, "claude-3-opus")
            .await;

        {
            let mut fetchers = router.usage_fetchers.write().await;
            fetchers.register("openai", Arc::new(MockUsageFetcher { remaining: 42.0 }));
            fetchers.register("anthropic", Arc::new(MockUsageFetcher { remaining: 7.0 }));
        }

        let snapshots = router.all_snapshots_with_quota().await;
        assert_eq!(snapshots.len(), 2);

        let openai = snapshots.iter().find(|s| s.provider == "openai").unwrap();
        let anthropic = snapshots
            .iter()
            .find(|s| s.provider == "anthropic")
            .unwrap();

        assert_eq!(openai.quota.as_ref().unwrap().remaining, 42.0);
        assert_eq!(anthropic.quota.as_ref().unwrap().remaining, 7.0);
    }
}
