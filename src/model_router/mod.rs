//! Model Router - Multi-provider LLM support with fallback chain
//!
//! Provides:
//! - Model aliases (e.g., "fast" -> "claude-3-haiku")
//! - Multi-provider routing (Anthropic, OpenAI, etc.)
//! - Automatic fallback on failure
//! - Health checking and load balancing

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::providers::{CompletionRequest, CompletionResponse, Message, Provider};

/// Model alias configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAlias {
    /// Alias name (e.g., "fast", "smart", "coding")
    pub name: String,
    /// Provider name (e.g., "anthropic", "openai")
    pub provider: String,
    /// Actual model ID (e.g., "claude-3-haiku-20240307")
    pub model: String,
    /// Temperature override (optional)
    pub temperature: Option<f32>,
    /// Max tokens override (optional)
    pub max_tokens: Option<u32>,
}

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider type
    pub provider_type: ProviderType,
    /// API key
    pub api_key: String,
    /// Base URL (for custom deployments)
    pub base_url: Option<String>,
    /// Request timeout
    pub timeout: Duration,
    /// Max retries
    pub max_retries: u32,
    /// Retry delay base
    pub retry_delay_ms: u64,
}

/// Supported provider types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Anthropic,
    OpenAi,
    Azure,
    Ollama,
    Custom { name: String },
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderType::Anthropic => write!(f, "anthropic"),
            ProviderType::OpenAi => write!(f, "openai"),
            ProviderType::Azure => write!(f, "azure"),
            ProviderType::Ollama => write!(f, "ollama"),
            ProviderType::Custom { name } => write!(f, "{}", name),
        }
    }
}

/// Fallback chain entry
#[derive(Debug, Clone)]
pub struct FallbackEntry {
    /// Provider name
    pub provider: String,
    /// Model ID
    pub model: String,
    /// Whether to use if primary fails
    pub enabled: bool,
    /// Health score (0-100)
    pub health_score: u8,
}

/// Model router configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRouterConfig {
    /// Default model alias
    pub default_model: String,
    /// Model aliases
    pub aliases: HashMap<String, ModelAlias>,
    /// Provider configurations
    pub providers: HashMap<String, ProviderConfig>,
    /// Fallback chain: alias -> ordered list of providers
    pub fallback_chains: HashMap<String, Vec<String>>,
    /// Health check interval
    pub health_check_interval_secs: u64,
    /// Circuit breaker threshold (failures before opening)
    pub circuit_breaker_threshold: u32,
    /// Circuit breaker reset timeout
    pub circuit_breaker_reset_secs: u64,
}

impl Default for ModelRouterConfig {
    fn default() -> Self {
        let mut aliases = HashMap::new();
        aliases.insert(
            "default".to_string(),
            ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3-sonnet-20240229".to_string(),
                temperature: None,
                max_tokens: None,
            },
        );
        aliases.insert(
            "fast".to_string(),
            ModelAlias {
                name: "fast".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3-haiku-20240307".to_string(),
                temperature: None,
                max_tokens: None,
            },
        );
        aliases.insert(
            "smart".to_string(),
            ModelAlias {
                name: "smart".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3-opus-20240229".to_string(),
                temperature: None,
                max_tokens: None,
            },
        );

        Self {
            default_model: "default".to_string(),
            aliases,
            providers: HashMap::new(),
            fallback_chains: HashMap::new(),
            health_check_interval_secs: 60,
            circuit_breaker_threshold: 5,
            circuit_breaker_reset_secs: 300,
        }
    }
}

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CircuitState {
    #[default]
    Closed,    // Normal operation
    Open,      // Failing, reject requests
    HalfOpen,  // Testing if recovered
}

/// Provider health tracking
#[derive(Debug, Clone)]
pub struct ProviderHealth {
    /// Current circuit state
    pub state: CircuitState,
    /// Consecutive failures
    pub failures: u32,
    /// Successful requests
    pub successes: u64,
    /// Last failure time
    pub last_failure: Option<chrono::DateTime<chrono::Utc>>,
    /// Average latency (ms)
    pub avg_latency_ms: u64,
    /// Last health check
    pub last_health_check: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for ProviderHealth {
    fn default() -> Self {
        Self {
            state: CircuitState::Closed,
            failures: 0,
            successes: 0,
            last_failure: None,
            avg_latency_ms: 0,
            last_health_check: None,
        }
    }
}

/// Model router for multi-provider LLM routing
pub struct ModelRouter {
    /// Configuration
    pub config: RwLock<ModelRouterConfig>,
    /// Provider instances
    providers: RwLock<HashMap<String, Arc<dyn Provider + Send + Sync>>>,
    /// Health tracking per provider
    health: RwLock<HashMap<String, ProviderHealth>>,
    /// Active fallback chains
    fallback_chains: RwLock<HashMap<String, Vec<FallbackEntry>>>,
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
        }
    }

    /// Initialize providers from config
    pub async fn initialize(&self) -> crate::Result<()> {
        let config = self.config.read().await;

        for (name, provider_config) in &config.providers {
            info!("Initializing provider: {}", name);

            let provider = self.create_provider(provider_config).await?;

            let mut providers = self.providers.write().await;
            providers.insert(name.clone(), provider);

            let mut health = self.health.write().await;
            health.insert(name.clone(), ProviderHealth::default());
        }

        // Initialize fallback chains
        let mut chains = self.fallback_chains.write().await;
        for (alias, provider_list) in &config.fallback_chains {
            let entries: Vec<FallbackEntry> = provider_list
                .iter()
                .map(|p| FallbackEntry {
                    provider: p.clone(),
                    model: config
                        .aliases
                        .get(alias)
                        .map(|a| a.model.clone())
                        .unwrap_or_default(),
                    enabled: true,
                    health_score: 100,
                })
                .collect();
            chains.insert(alias.clone(), entries);
        }

        Ok(())
    }

    /// Start the health check background task
    pub fn start_health_checks(self: Arc<Self>) {
        tokio::spawn(async move {
            let interval_secs = {
                let config = self.config.read().await;
                config.health_check_interval_secs
            };
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            loop {
                interval.tick().await;
                self.run_health_checks().await;
            }
        });
    }

    /// Create a provider instance from config
    async fn create_provider(
        &self,
        config: &ProviderConfig,
    ) -> crate::Result<Arc<dyn Provider + Send + Sync>> {
        match config.provider_type {
            ProviderType::Anthropic => {
                // Create Anthropic provider (with optional custom base_url for Kimi, etc.)
                let provider = if let Some(ref base_url) = config.base_url {
                    crate::providers::anthropic::AnthropicProvider::with_base_url(
                        config.api_key.clone(),
                        base_url.clone(),
                    )?
                } else {
                    crate::providers::anthropic::AnthropicProvider::new(
                        config.api_key.clone(),
                    )?
                };
                Ok(Arc::new(provider))
            }
            ProviderType::OpenAi => {
                // Create OpenAI provider
                let base_url = config.base_url.clone().unwrap_or_else(||
                    "https://api.openai.com/v1".to_string()
                );
                let provider = crate::providers::OpenAiProvider::with_base_url(
                    config.api_key.clone(),
                    base_url,
                )?;
                Ok(Arc::new(provider))
            }
            _ => Err(crate::error::ConfigError::InvalidValue {
                key: "provider_type".to_string(),
                message: format!("Provider type not supported: {:?}", config.provider_type),
            }.into()),
        }
    }

    /// Complete a request using the model router
    pub async fn complete(
        &self,
        alias_or_model: &str,
        messages: Vec<Message>,
    ) -> crate::Result<CompletionResponse> {
        // Resolve alias
        let config = self.config.read().await;
        let alias = config
            .aliases
            .get(alias_or_model)
            .or_else(|| config.aliases.get(&config.default_model))
            .cloned()
            .ok_or_else(|| crate::error::ConfigError::InvalidValue {
                key: "model_alias".to_string(),
                message: format!("Unknown model alias: {}", alias_or_model),
            })?;
        drop(config);

        // Build request
        let request = CompletionRequest {
            model: Some(alias.model.clone()),
            messages,
            temperature: alias.temperature,
            max_tokens: alias.max_tokens,
            stream: false,
            tools: None,
            stop: None,
        };

        // Try primary provider, then fallbacks
        let providers_to_try = self.get_provider_chain(&alias).await;

        let mut last_error = None;

        for entry in providers_to_try {
            if !entry.enabled {
                continue;
            }

            // Check circuit breaker
            if self.is_circuit_open(&entry.provider).await {
                warn!(
                    "Circuit breaker open for provider: {}",
                    entry.provider
                );
                continue;
            }

            let providers = self.providers.read().await;
            if let Some(provider) = providers.get(&entry.provider) {
                let start = std::time::Instant::now();

                match provider.complete(request.clone()).await {
                    Ok(response) => {
                        // Record success
                        self.record_success(&entry.provider, start.elapsed()).await;
                        return Ok(response);
                    }
                    Err(e) => {
                        error!(
                            "Provider {} failed: {}",
                            entry.provider, e
                        );
                        self.record_failure(&entry.provider).await;
                        last_error = Some(e);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| crate::error::MantaError::ExternalService {
            source: "All providers failed".to_string(),
            cause: None,
        }))
    }

    /// Get the ordered list of providers to try
    async fn get_provider_chain(&self, alias: &ModelAlias) -> Vec<FallbackEntry> {
        let chains = self.fallback_chains.read().await;

        if let Some(chain) = chains.get(&alias.name) {
            return chain.clone();
        }

        // Default: just the primary provider
        vec![FallbackEntry {
            provider: alias.provider.clone(),
            model: alias.model.clone(),
            enabled: true,
            health_score: 100,
        }]
    }

    /// Check if circuit breaker is open for a provider
    async fn is_circuit_open(&self, provider: &str) -> bool {
        let health = self.health.read().await;
        if let Some(h) = health.get(provider) {
            h.state == CircuitState::Open
        } else {
            false
        }
    }

    /// Record a successful request
    async fn record_success(&self, provider: &str, latency: Duration) {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(provider) {
            h.successes += 1;
            h.failures = 0;
            h.state = CircuitState::Closed;

            // Update average latency (exponential moving average)
            let latency_ms = latency.as_millis() as u64;
            h.avg_latency_ms = (h.avg_latency_ms * 9 + latency_ms) / 10;
        }
    }

    /// Record a failed request
    async fn record_failure(&self, provider: &str) {
        let config = self.config.read().await;
        let threshold = config.circuit_breaker_threshold;
        drop(config);

        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(provider) {
            h.failures += 1;
            h.last_failure = Some(chrono::Utc::now());

            if h.failures >= threshold && h.state == CircuitState::Closed {
                warn!(
                    "Circuit breaker opened for provider: {} ({} failures)",
                    provider, h.failures
                );
                h.state = CircuitState::Open;
            }
        }
    }

    /// Run periodic health checks
    async fn run_health_checks(&self) {
        let providers = self.providers.read().await;
        let provider_names: Vec<String> = providers.keys().cloned().collect();
        drop(providers);

        for name in provider_names {
            // TODO: Implement actual health check with lightweight request
            // For now, just update timestamp
            let mut health = self.health.write().await;
            if let Some(h) = health.get_mut(&name) {
                h.last_health_check = Some(chrono::Utc::now());

                // Check if we should transition from Open to HalfOpen
                if h.state == CircuitState::Open {
                    if let Some(last_failure) = h.last_failure {
                        let elapsed = chrono::Utc::now() - last_failure;
                        let config = self.config.read().await;
                        if elapsed.num_seconds() >= config.circuit_breaker_reset_secs as i64 {
                            info!("Circuit breaker half-open for provider: {}", name);
                            h.state = CircuitState::HalfOpen;
                        }
                    }
                }
            }
        }
    }

    /// Get health status for all providers
    pub async fn get_health_status(&self) -> HashMap<String, ProviderHealth> {
        let health = self.health.read().await;
        health
            .iter()
            .map(|(k, v)| (k.clone(), ProviderHealth {
                state: v.state,
                failures: v.failures,
                successes: v.successes,
                last_failure: v.last_failure,
                avg_latency_ms: v.avg_latency_ms,
                last_health_check: v.last_health_check,
            }))
            .collect()
    }

    /// Create a default provider (first available)
    pub async fn create_default_provider(&self) -> crate::Result<Arc<dyn Provider + Send + Sync>> {
        let providers = self.providers.read().await;

        // Try to get the first provider
        if let Some((name, provider)) = providers.iter().next() {
            info!("Using default provider: {}", name);
            Ok(provider.clone())
        } else {
            // No providers configured - create a default Anthropic provider from env
            drop(providers);

            if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
                info!("Creating default Anthropic provider from environment");
                let provider = crate::providers::anthropic::AnthropicProvider::new(api_key)?;
                let provider_arc = Arc::new(provider);

                // Store it for future use
                let mut providers = self.providers.write().await;
                providers.insert("anthropic".to_string(), provider_arc.clone());

                Ok(provider_arc)
            } else {
                Err(crate::error::ConfigError::Missing(
                    "No providers configured and ANTHROPIC_API_KEY not set".to_string()
                ).into())
            }
        }
    }

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

    // ==================== RUNTIME PROVIDER MANAGEMENT ====================

    /// Switch the default model alias
    pub async fn switch_default_model(&self, alias_name: &str) -> crate::Result<()> {
        let config = self.config.read().await;
        if !config.aliases.contains_key(alias_name) {
            return Err(crate::error::ConfigError::InvalidValue {
                key: "default_model".to_string(),
                message: format!("Unknown model alias: {}", alias_name),
            }.into());
        }
        drop(config);

        let mut config = self.config.write().await;
        info!("Switching default model from '{}' to '{}'", config.default_model, alias_name);
        config.default_model = alias_name.to_string();
        Ok(())
    }

    /// Get current default model alias
    pub async fn get_default_model(&self) -> String {
        let config = self.config.read().await;
        config.default_model.clone()
    }

    /// List all available providers with their status
    pub async fn list_providers(&self) -> Vec<ProviderInfo> {
        let providers = self.providers.read().await;
        let health = self.health.read().await;
        let config = self.config.read().await;

        providers
            .iter()
            .map(|(name, _)| {
                let h = health.get(name).cloned().unwrap_or_default();
                let provider_config = config.providers.get(name).cloned();

                ProviderInfo {
                    name: name.clone(),
                    provider_type: provider_config.as_ref().map(|c| format!("{:?}", c.provider_type)).unwrap_or_default(),
                    enabled: h.state != CircuitState::Open,
                    health: ProviderHealthInfo {
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

    /// Enable a provider (close circuit breaker if open)
    pub async fn enable_provider(&self, name: &str) -> crate::Result<()> {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(name) {
            h.state = CircuitState::Closed;
            h.failures = 0;
            info!("Provider {} enabled (circuit closed)", name);
            Ok(())
        } else {
            Err(crate::error::ConfigError::InvalidValue {
                key: "provider".to_string(),
                message: format!("Unknown provider: {}", name),
            }.into())
        }
    }

    /// Disable a provider (open circuit breaker)
    pub async fn disable_provider(&self, name: &str) -> crate::Result<()> {
        let mut health = self.health.write().await;
        if let Some(h) = health.get_mut(name) {
            h.state = CircuitState::Open;
            info!("Provider {} disabled (circuit opened)", name);
            Ok(())
        } else {
            Err(crate::error::ConfigError::InvalidValue {
                key: "provider".to_string(),
                message: format!("Unknown provider: {}", name),
            }.into())
        }
    }

    /// Add a new provider at runtime
    pub async fn add_provider(&self, name: &str, config: ProviderConfig) -> crate::Result<()> {
        info!("Adding new provider at runtime: {}", name);

        // Create provider instance
        let provider = self.create_provider(&config).await?;

        // Add to providers
        let mut providers = self.providers.write().await;
        providers.insert(name.to_string(), provider);
        drop(providers);

        // Add to health tracking
        let mut health = self.health.write().await;
        health.insert(name.to_string(), ProviderHealth::default());
        drop(health);

        // Add to config
        let mut router_config = self.config.write().await;
        router_config.providers.insert(name.to_string(), config);

        Ok(())
    }

    /// Remove a provider at runtime
    pub async fn remove_provider(&self, name: &str) -> crate::Result<()> {
        info!("Removing provider at runtime: {}", name);

        let mut providers = self.providers.write().await;
        if providers.remove(name).is_none() {
            return Err(crate::error::ConfigError::InvalidValue {
                key: "provider".to_string(),
                message: format!("Unknown provider: {}", name),
            }.into());
        }
        drop(providers);

        let mut health = self.health.write().await;
        health.remove(name);
        drop(health);

        let mut config = self.config.write().await;
        config.providers.remove(name);

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
        let providers = self.providers.read().await;
        let provider = providers.get(name).cloned().ok_or_else(|| {
            crate::error::ConfigError::InvalidValue {
                key: "provider".to_string(),
                message: format!("Unknown provider: {}", name),
            }
        })?;
        drop(providers);

        // Perform lightweight health check
        // For now, just check if provider responds
        let request = CompletionRequest {
            model: None,
            messages: vec![Message::system("Health check")],
            temperature: Some(0.0),
            max_tokens: Some(1),
            stream: false,
            tools: None,
            stop: None,
        };

        let start = std::time::Instant::now();
        match provider.complete(request).await {
            Ok(_) => {
                self.record_success(name, start.elapsed()).await;
                Ok(true)
            }
            Err(_) => {
                self.record_failure(name).await;
                Ok(false)
            }
        }
    }

    /// Complete a request with a specific provider override (per-request override)
    pub async fn complete_with_provider(
        &self,
        provider_name: &str,
        model: Option<String>,
        messages: Vec<Message>,
    ) -> crate::Result<CompletionResponse> {
        let providers = self.providers.read().await;
        let provider = providers.get(provider_name).cloned().ok_or_else(|| {
            crate::error::ConfigError::InvalidValue {
                key: "provider".to_string(),
                message: format!("Unknown provider: {}", provider_name),
            }
        })?;
        drop(providers);

        // Check circuit breaker
        if self.is_circuit_open(provider_name).await {
            return Err(crate::error::MantaError::ExternalService {
                source: format!("Provider {} circuit is open", provider_name),
                cause: None,
            });
        }

        let request = CompletionRequest {
            model,
            messages,
            temperature: None,
            max_tokens: None,
            stream: false,
            tools: None,
            stop: None,
        };

        let start = std::time::Instant::now();
        match provider.complete(request).await {
            Ok(response) => {
                self.record_success(provider_name, start.elapsed()).await;
                Ok(response)
            }
            Err(e) => {
                self.record_failure(provider_name).await;
                Err(e)
            }
        }
    }

    /// Get fallback chain for an alias
    pub async fn get_fallback_chain(&self, alias_name: &str) -> Vec<String> {
        let chains = self.fallback_chains.read().await;
        chains.get(alias_name)
            .map(|entries| entries.iter().map(|e| e.provider.clone()).collect())
            .unwrap_or_default()
    }

    /// Update fallback chain for an alias at runtime
    pub async fn set_fallback_chain(&self, alias_name: &str, provider_chain: Vec<String>) -> crate::Result<()> {
        let config = self.config.read().await;
        if !config.aliases.contains_key(alias_name) {
            return Err(crate::error::ConfigError::InvalidValue {
                key: "alias".to_string(),
                message: format!("Unknown alias: {}", alias_name),
            }.into());
        }
        let model = config.aliases.get(alias_name).map(|a| a.model.clone()).unwrap_or_default();
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

        let mut chains = self.fallback_chains.write().await;
        chains.insert(alias_name.to_string(), entries);

        // Also update config
        let mut config = self.config.write().await;
        config.fallback_chains.insert(alias_name.to_string(), provider_chain);

        Ok(())
    }
}

/// Provider information for API responses
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    /// Provider name
    pub name: String,
    /// Provider type (anthropic, openai, etc.)
    pub provider_type: String,
    /// Whether provider is enabled
    pub enabled: bool,
    /// Health information
    pub health: ProviderHealthInfo,
    /// Circuit breaker state (internal use)
    #[serde(skip)]
    pub circuit_state: CircuitState,
}

/// Provider health information for API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealthInfo {
    /// Circuit state (Closed, Open, HalfOpen)
    pub state: String,
    /// Consecutive failures
    pub failures: u32,
    /// Successful requests
    pub successes: u64,
    /// Average latency in ms
    pub avg_latency_ms: u64,
    /// Last failure timestamp
    pub last_failure: Option<chrono::DateTime<chrono::Utc>>,
    /// Last health check timestamp
    pub last_health_check: Option<chrono::DateTime<chrono::Utc>>,
}

/// Trait for LLM providers
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Get provider name
    fn name(&self) -> &str;

    /// Get available models
    async fn list_models(&self) -> crate::Result<Vec<String>>;

    /// Complete a chat request
    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> crate::Result<CompletionResponse>;

    /// Stream a completion
    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> crate::Result<tokio::sync::mpsc::Receiver<crate::Result<CompletionResponse>>>;

    /// Health check
    async fn health_check(&self) -> crate::Result<bool>;
}
