//! Model Router - Multi-provider LLM support with fallback chain
//!
//! Provides:
//! - Model aliases (e.g., "fast" -> "claude-3-haiku")
//! - Multi-provider routing (Anthropic, OpenAI, etc.)
//! - Automatic fallback on failure
//! - Health checking and load balancing
//! - Auth profile rotation with cooldown

pub mod auth_profile;
pub mod auth_profile_store;
pub mod failure_class;
pub mod gateway_client;
pub mod model_catalog;
pub mod oauth_callback;
pub mod oauth_credential;
pub mod oauth_flow;
pub mod pkce;
pub mod usage_fetcher;
pub mod usage_formatter;
pub mod usage_tracker;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
pub use auth_profile::{
    AuthProfile, AuthProfileConfig, AuthProfileManager, KeyStatus, ProfileStatus,
};
pub use auth_profile_store::AuthProfileStore;
use chrono::Utc;
pub use failure_class::FailureClass;
pub use gateway_client::{GatewayClient, HttpGatewayClient};
pub use model_catalog::{ModelCatalog, ModelCatalogEntry, ModelDiscoverySource, ModelPricing};
pub use oauth_callback::wait_for_callback;
pub use oauth_credential::Credential;
pub use oauth_flow::OAuthFlow;
pub use pkce::{challenge_from_verifier, generate_verifier};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
pub use usage_fetcher::{
    LocalBudgetFetcher, OpenAiUsageFetcher, UsageFetcher, UsageFetcherRegistry,
};
pub use usage_formatter::{
    format_provider_snapshot, format_tokens, format_usage_report, format_usage_summary_line,
    format_window, format_window_compact,
};
pub use usage_tracker::{
    ProviderUsageSnapshot, ProviderUsageTracker, QuotaSource, UsageQuota, UsageTrackerConfig,
};

use crate::providers::{
    CompletionRequest, CompletionResponse, CompletionStream, Message, Provider, ToolDefinition,
};

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

/// OAuth 2.0 configuration for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// OAuth2 client ID
    pub client_id: String,
    /// Authorization endpoint URL
    pub auth_url: String,
    /// Token endpoint URL
    pub token_url: String,
    /// Optional scope string
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Local redirect callback port (default: 18081)
    #[serde(default = "default_redirect_port")]
    pub redirect_port: u16,
}

fn default_redirect_port() -> u16 {
    18081
}

/// Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider type
    pub provider_type: ProviderType,
    /// API key (single key, backward compatible)
    #[serde(alias = "api_key")]
    pub api_key: String,
    /// Multiple API keys for rotation (optional, takes precedence over api_key)
    #[serde(default, alias = "api_keys")]
    pub api_keys: Vec<String>,
    /// Auth profile configuration (optional, most flexible)
    #[serde(default, alias = "auth_profile")]
    pub auth_profile: Option<AuthProfileConfig>,
    /// OAuth 2.0 configuration for initial authorization flow
    #[serde(default, alias = "oauth")]
    pub oauth: Option<OAuthConfig>,
    /// Base URL (for custom deployments)
    pub base_url: Option<String>,
    /// Request timeout
    #[serde(default = "default_timeout")]
    pub timeout: Duration,
    /// Max retries
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Retry delay base
    #[serde(default = "default_retry_delay_ms")]
    pub retry_delay_ms: u64,
}

fn default_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_max_retries() -> u32 {
    3
}

fn default_retry_delay_ms() -> u64 {
    1000
}

impl ProviderConfig {
    /// Get the effective API key to use for provider creation.
    /// Prefers auth_profile keys, then api_keys, then single api_key.
    pub fn effective_key(&self) -> String {
        if let Some(ref profile) = self.auth_profile {
            if let Some(first) = profile.keys.first() {
                return first.key.clone();
            }
        }
        if let Some(first) = self.api_keys.first() {
            return first.clone();
        }
        self.api_key.clone()
    }

    /// Build an AuthProfileConfig from this config if one is not explicitly
    /// set.
    pub fn derived_auth_profile_config(&self) -> AuthProfileConfig {
        if let Some(ref profile) = self.auth_profile {
            return profile.clone();
        }
        let mut keys = Vec::new();
        if !self.api_key.is_empty() {
            keys.push(auth_profile::AuthKeyConfig {
                key: self.api_key.clone(),
                label: "primary".to_string(),
            });
        }
        for (i, key) in self.api_keys.iter().enumerate() {
            if i == 0 && key == &self.api_key {
                continue; // avoid duplicate
            }
            keys.push(auth_profile::AuthKeyConfig {
                key: key.clone(),
                label: format!("key-{}", i),
            });
        }
        AuthProfileConfig {
            keys,
            cooldown_secs: 60,
            max_failures: 3,
        }
    }
}

/// Cost information for a model (per 1K tokens in USD)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCost {
    /// Input cost per 1K tokens
    pub input_cost_per_1k: f64,
    /// Output cost per 1K tokens
    pub output_cost_per_1k: f64,
}

impl ModelCost {
    /// Estimate cost for a given usage
    pub fn estimate(&self, usage: &crate::providers::Usage) -> f64 {
        let input = usage.prompt_tokens as f64 * self.input_cost_per_1k / 1000.0;
        let output = usage.completion_tokens as f64 * self.output_cost_per_1k / 1000.0;
        input + output
    }
}

/// Task type classification for cost-aware routing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    /// Complex code generation or refactoring
    Coding,
    /// Multi-step logical reasoning
    Reasoning,
    /// Creative writing, storytelling
    Creative,
    /// Summarizing long content
    Summarization,
    /// Simple categorization or labeling
    Classification,
    /// Structured data extraction
    Extraction,
    /// Language translation
    Translation,
    /// General conversation
    Chat,
    /// Default / fallback
    Unknown,
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self)
                .unwrap_or_default()
                .trim_matches('"')
        )
    }
}

/// Task-to-model routing rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRoutingRule {
    /// Task type this rule applies to
    pub task_type: TaskType,
    /// Preferred model alias
    pub preferred_alias: String,
    /// Fallback alias if preferred is unavailable
    pub fallback_alias: Option<String>,
    /// Maximum input tokens for this rule (route to larger model if exceeded)
    pub max_input_tokens: Option<u32>,
}

/// Cost-aware routing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostAwareConfig {
    /// Whether cost-aware routing is enabled
    pub enabled: bool,
    /// Cost per 1K tokens for each model alias
    pub model_costs: HashMap<String, ModelCost>,
    /// Routing rules by task type
    pub routing_rules: Vec<TaskRoutingRule>,
    /// Default alias when no rule matches
    pub default_alias: String,
    /// Optional daily budget limit in USD
    pub budget_limit_usd: Option<f64>,
    /// Current daily spend (reset at midnight UTC)
    #[serde(skip)]
    pub daily_spend_usd: f64,
}

impl Default for CostAwareConfig {
    fn default() -> Self {
        let mut model_costs = HashMap::new();
        // Default costs (approximate, should be updated with actual pricing)
        model_costs.insert(
            "fast".to_string(),
            ModelCost {
                input_cost_per_1k: 0.25,
                output_cost_per_1k: 1.25,
            },
        );
        model_costs.insert(
            "default".to_string(),
            ModelCost {
                input_cost_per_1k: 3.0,
                output_cost_per_1k: 15.0,
            },
        );
        model_costs.insert(
            "smart".to_string(),
            ModelCost {
                input_cost_per_1k: 15.0,
                output_cost_per_1k: 75.0,
            },
        );

        let routing_rules = vec![
            TaskRoutingRule {
                task_type: TaskType::Coding,
                preferred_alias: "smart".to_string(),
                fallback_alias: Some("default".to_string()),
                max_input_tokens: Some(8000),
            },
            TaskRoutingRule {
                task_type: TaskType::Reasoning,
                preferred_alias: "smart".to_string(),
                fallback_alias: Some("default".to_string()),
                max_input_tokens: None,
            },
            TaskRoutingRule {
                task_type: TaskType::Classification,
                preferred_alias: "fast".to_string(),
                fallback_alias: Some("default".to_string()),
                max_input_tokens: Some(4000),
            },
            TaskRoutingRule {
                task_type: TaskType::Summarization,
                preferred_alias: "default".to_string(),
                fallback_alias: Some("fast".to_string()),
                max_input_tokens: Some(16000),
            },
            TaskRoutingRule {
                task_type: TaskType::Extraction,
                preferred_alias: "fast".to_string(),
                fallback_alias: Some("default".to_string()),
                max_input_tokens: Some(8000),
            },
            TaskRoutingRule {
                task_type: TaskType::Translation,
                preferred_alias: "fast".to_string(),
                fallback_alias: Some("default".to_string()),
                max_input_tokens: None,
            },
            TaskRoutingRule {
                task_type: TaskType::Creative,
                preferred_alias: "default".to_string(),
                fallback_alias: Some("smart".to_string()),
                max_input_tokens: None,
            },
            TaskRoutingRule {
                task_type: TaskType::Chat,
                preferred_alias: "default".to_string(),
                fallback_alias: Some("fast".to_string()),
                max_input_tokens: Some(4000),
            },
            TaskRoutingRule {
                task_type: TaskType::Unknown,
                preferred_alias: "default".to_string(),
                fallback_alias: Some("fast".to_string()),
                max_input_tokens: None,
            },
        ];

        Self {
            enabled: false,
            model_costs,
            routing_rules,
            default_alias: "default".to_string(),
            budget_limit_usd: None,
            daily_spend_usd: 0.0,
        }
    }
}

/// Lightweight rule-based task classifier
pub struct TaskClassifier;

impl TaskClassifier {
    /// Classify a conversation into a task type based on message content
    pub fn classify(messages: &[crate::providers::Message]) -> TaskType {
        use crate::providers::Role;

        let text: String = messages
            .iter()
            .filter(|m| matches!(m.role, Role::User))
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        let lower = text.to_lowercase();

        // Coding patterns
        if lower.contains("code")
            || lower.contains("function")
            || lower.contains("bug")
            || lower.contains("refactor")
            || lower.contains("implement")
            || lower.contains("debug")
            || lower.contains("programming")
            || lower.contains("algorithm")
            || (lower.starts_with("write a") && lower.contains("script"))
            || lower.contains("```")
        {
            return TaskType::Coding;
        }

        // Reasoning patterns
        if lower.contains("explain why")
            || lower.contains("analyze")
            || lower.contains("compare")
            || lower.contains("evaluate")
            || lower.contains("reason")
            || lower.contains("logic")
            || lower.contains("step by step")
            || lower.contains("prove")
            || lower.contains("why does")
            || lower.contains("how does")
        {
            return TaskType::Reasoning;
        }

        // Summarization
        if lower.contains("summarize")
            || lower.contains("summary")
            || lower.contains("tl;dr")
            || lower.contains("key points")
            || lower.contains("main ideas")
        {
            return TaskType::Summarization;
        }

        // Classification
        if lower.contains("classify")
            || lower.contains("categor")
            || lower.contains("label")
            || lower.contains("sentiment")
            || lower.contains("is this")
            || lower.starts_with("what type")
        {
            return TaskType::Classification;
        }

        // Translation
        if lower.contains("translate") || lower.contains("translation") {
            return TaskType::Translation;
        }

        // Extraction
        if lower.contains("extract")
            || lower.contains("pull out")
            || lower.contains("parse")
            || lower.contains("find all")
            || lower.contains("list the")
            || lower.contains("get the")
        {
            return TaskType::Extraction;
        }

        // Creative
        if lower.contains("write a story")
            || lower.contains("poem")
            || lower.contains("creative")
            || lower.contains("draft")
            || lower.contains("compose")
            || lower.contains("rewrite")
        {
            return TaskType::Creative;
        }

        TaskType::Chat
    }
}

/// Supported provider types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Anthropic,
    OpenAi,
    Azure,
    Ollama,
    Gemini,
    Moonshot,
    Minimax,
    Custom { name: String },
}

/// Preset for a known LLM provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPreset {
    /// Display name (e.g. "DeepSeek")
    pub display_name: String,
    /// Underlying protocol
    pub protocol: ProviderType,
    /// Default base URL (optional for native providers)
    pub default_base_url: Option<String>,
    /// Suggested model IDs
    pub models: Vec<String>,
}

/// Built-in provider presets keyed by provider name.
pub fn provider_presets() -> HashMap<String, ProviderPreset> {
    let mut m = HashMap::new();
    m.insert(
        "anthropic".to_string(),
        ProviderPreset {
            display_name: "Anthropic".to_string(),
            protocol: ProviderType::Anthropic,
            default_base_url: None,
            models: vec![
                "claude-sonnet-4-20250514".to_string(),
                "claude-3-5-sonnet-20241022".to_string(),
                "claude-3-opus-20240229".to_string(),
                "claude-3-sonnet-20240229".to_string(),
                "claude-3-haiku-20240307".to_string(),
            ],
        },
    );
    m.insert(
        "openai".to_string(),
        ProviderPreset {
            display_name: "OpenAI".to_string(),
            protocol: ProviderType::OpenAi,
            default_base_url: Some("https://api.openai.com/v1".to_string()),
            models: vec![
                "gpt-4o".to_string(),
                "gpt-4o-mini".to_string(),
                "gpt-4-turbo".to_string(),
                "gpt-3.5-turbo".to_string(),
            ],
        },
    );
    m.insert(
        "deepseek".to_string(),
        ProviderPreset {
            display_name: "DeepSeek".to_string(),
            protocol: ProviderType::OpenAi,
            default_base_url: Some("https://api.deepseek.com/v1".to_string()),
            models: vec!["deepseek-chat".to_string(), "deepseek-reasoner".to_string()],
        },
    );
    m.insert(
        "qwen".to_string(),
        ProviderPreset {
            display_name: "Qwen".to_string(),
            protocol: ProviderType::OpenAi,
            default_base_url: Some("https://dashscope.aliyuncs.com/compatible-mode/v1".to_string()),
            models: vec![
                "qwen-max".to_string(),
                "qwen-plus".to_string(),
                "qwen-turbo".to_string(),
            ],
        },
    );
    m.insert(
        "kimi".to_string(),
        ProviderPreset {
            display_name: "Kimi".to_string(),
            protocol: ProviderType::Moonshot,
            default_base_url: None,
            models: vec![
                "kimi-k2".to_string(),
                "kimi-moonshot-v1-8k".to_string(),
                "kimi-moonshot-v1-32k".to_string(),
                "kimi-moonshot-v1-128k".to_string(),
            ],
        },
    );
    m.insert(
        "gemini".to_string(),
        ProviderPreset {
            display_name: "Gemini".to_string(),
            protocol: ProviderType::Gemini,
            default_base_url: None,
            models: vec![
                "gemini-1.5-pro".to_string(),
                "gemini-1.5-flash".to_string(),
                "gemini-2.5-pro-preview-03-25".to_string(),
            ],
        },
    );
    m.insert(
        "minimax".to_string(),
        ProviderPreset {
            display_name: "MiniMax".to_string(),
            protocol: ProviderType::Minimax,
            default_base_url: None,
            models: vec!["abab6.5s-chat".to_string(), "abab6-chat".to_string()],
        },
    );
    m.insert(
        "azure".to_string(),
        ProviderPreset {
            display_name: "Azure OpenAI".to_string(),
            protocol: ProviderType::Azure,
            default_base_url: None,
            models: vec![
                "gpt-4o".to_string(),
                "gpt-4".to_string(),
                "gpt-35-turbo".to_string(),
            ],
        },
    );
    m.insert(
        "ollama".to_string(),
        ProviderPreset {
            display_name: "Ollama".to_string(),
            protocol: ProviderType::Ollama,
            default_base_url: Some("http://localhost:11434".to_string()),
            models: vec![
                "llama3".to_string(),
                "llama3.1".to_string(),
                "mistral".to_string(),
                "qwen2".to_string(),
            ],
        },
    );
    m.insert(
        "custom".to_string(),
        ProviderPreset {
            display_name: "Custom".to_string(),
            protocol: ProviderType::Custom { name: "custom".to_string() },
            default_base_url: None,
            models: vec![],
        },
    );
    m
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderType::Anthropic => write!(f, "anthropic"),
            ProviderType::OpenAi => write!(f, "openai"),
            ProviderType::Azure => write!(f, "azure"),
            ProviderType::Ollama => write!(f, "ollama"),
            ProviderType::Gemini => write!(f, "gemini"),
            ProviderType::Moonshot => write!(f, "moonshot"),
            ProviderType::Minimax => write!(f, "minimax"),
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
    /// Cost-aware routing configuration
    #[serde(default)]
    pub cost_aware: Option<CostAwareConfig>,
}

impl Default for ModelRouterConfig {
    fn default() -> Self {
        Self {
            default_model: String::new(),
            aliases: HashMap::new(),
            providers: HashMap::new(),
            fallback_chains: HashMap::new(),
            health_check_interval_secs: 60,
            circuit_breaker_threshold: 5,
            circuit_breaker_reset_secs: 300,
            cost_aware: None,
        }
    }
}

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CircuitState {
    #[default]
    Closed, // Normal operation
    Open,     // Failing, reject requests
    HalfOpen, // Testing if recovered
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
        }
    }

    /// Attach a SQLite connection pool for persisting auth profile state.
    pub fn with_db_pool(mut self, pool: sqlx::Pool<sqlx::Sqlite>) -> Self {
        self.db_pool = Some(pool);
        self
    }

    /// Initialize providers from config
    pub async fn initialize(&self) -> crate::Result<()> {
        // Wire up persistent store if a database pool is available
        if let Some(ref pool) = self.db_pool {
            let store = std::sync::Arc::new(AuthProfileStore::new(pool.clone()));
            self.auth_profiles.set_store(store).await;
        }

        let config = self.config.read().await;

        for (name, provider_config) in &config.providers {
            info!("Initializing provider: {}", name);

            // Register auth profile for this provider (loads persisted state if store is
            // set)
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

            // Register remote usage fetcher for supported providers
            if matches!(provider_config.provider_type, ProviderType::OpenAi) {
                let api_key = provider_config.effective_key();
                if !api_key.is_empty() {
                    let fetcher = OpenAiUsageFetcher::new(api_key);
                    let mut fetchers = self.usage_fetchers.write().await;
                    fetchers.register(name.clone(), Box::new(fetcher));
                }
            }
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

        // Initialize model catalog from static aliases
        let alias_tuples: Vec<(String, String, String)> = config
            .aliases
            .values()
            .map(|a| (a.name.clone(), a.provider.clone(), a.model.clone()))
            .collect();
        drop(config); // release read lock before async catalog ops

        self.model_catalog
            .add_source(Box::new(model_catalog::StaticModelSource::new(alias_tuples)))
            .await;
        if let Err(e) = self.model_catalog.discover().await {
            warn!("Model catalog discovery failed: {}", e);
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
        let api_key = config.effective_key();
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

    /// Rebuild a provider with the current auth profile key after rotation.
    ///
    /// `cooldown_secs` overrides the default cooldown for this rotation.
    /// When `None`, the provider's configured cooldown is used.
    async fn rebuild_provider_with_rotated_key(
        &self,
        provider_name: &str,
        cooldown_secs: Option<u64>,
    ) -> crate::Result<()> {
        let config = {
            let cfg = self.config.read().await;
            cfg.providers.get(provider_name).cloned().ok_or_else(|| {
                crate::error::ConfigError::InvalidValue {
                    key: "provider".to_string(),
                    message: format!("Unknown provider: {}", provider_name),
                }
            })?
        };

        let cooldown =
            cooldown_secs.unwrap_or_else(|| config.derived_auth_profile_config().cooldown_secs);

        // Rotate to next key
        if let Some(new_key) = self.auth_profiles.rotate(provider_name, cooldown).await {
            let mut new_config = config;
            new_config.api_key = new_key.clone();
            new_config.api_keys = vec![new_key];
            new_config.auth_profile = None;

            // Rebuild provider with new key
            let provider = self.create_provider(&new_config).await?;
            let mut providers = self.providers.write().await;
            providers.insert(provider_name.to_string(), provider);

            // Update config
            let mut router_config = self.config.write().await;
            router_config
                .providers
                .insert(provider_name.to_string(), new_config);

            info!("Rebuilt provider '{}' with rotated API key", provider_name);
            Ok(())
        } else {
            Err(crate::error::SyscityError::ExternalService {
                source: format!(
                    "No available API keys for provider '{}' after rotation",
                    provider_name
                ),
                cause: None,
            })
        }
    }

    /// Compute the effective cooldown for a failure class, never below the
    /// provider's configured minimum.
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

    /// Complete a request using the model router
    pub async fn complete(
        &self,
        alias_or_model: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
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
            tools,
            stop: None,
            extra: None,
            requires_vision: false,
            requires_tools: false,
            requires_reasoning: false,
            ..Default::default()
        };

        // Capability-aware routing: upgrade alias if needed
        let alias = self.resolve_alias_with_capabilities(&alias, &request).await;

        // Try primary provider, then fallbacks
        let mut providers_to_try = self.get_provider_chain(&alias).await;

        // Append request-level fallback models if specified
        for fallback in &request.fallback_models {
            let config = self.config.read().await;
            if let Some(fb_alias) = config.aliases.get(fallback).cloned() {
                drop(config);
                let fb_chain = self.get_provider_chain(&fb_alias).await;
                for entry in fb_chain {
                    // Avoid duplicate provider+model combinations
                    if !providers_to_try
                        .iter()
                        .any(|e| e.provider == entry.provider && e.model == entry.model)
                    {
                        providers_to_try.push(entry);
                    }
                }
            } else {
                drop(config);
            }
        }

        let mut last_error = None;

        for entry in providers_to_try {
            if !entry.enabled {
                continue;
            }

            // Check circuit breaker
            if self.is_circuit_open(&entry.provider).await {
                warn!("Circuit breaker open for provider: {}", entry.provider);
                continue;
            }

            let providers = self.providers.read().await;
            if let Some(provider) = providers.get(&entry.provider) {
                let start = std::time::Instant::now();

                match provider.complete(request.clone()).await {
                    Ok(response) => {
                        // Record success on both health and auth profile
                        self.record_success(&entry.provider, start.elapsed()).await;
                        self.auth_profiles.record_success(&entry.provider).await;
                        if let Some(usage) = response.usage {
                            self.usage_tracker
                                .record(&entry.provider, usage, &alias.model)
                                .await;
                        }
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
                        drop(providers);

                        // Auto-suppress model on permanent failures or model-not-found
                        if class == FailureClass::ModelNotFound {
                            self.model_catalog
                                .suppress(&entry.provider, &entry.model)
                                .await;
                            warn!("Auto-suppressed model {}:{}", entry.provider, entry.model);
                        }

                        if class.should_disable_key() {
                            // Permanently disable the current key
                            let cooldown = self.cooldown_for_failure(&entry.provider, class).await;
                            if let Err(disable_err) = self
                                .rebuild_provider_with_rotated_key(&entry.provider, Some(cooldown))
                                .await
                            {
                                error!(
                                    "Key disable/rotation failed for provider {}: {}",
                                    entry.provider, disable_err
                                );
                            }
                            self.record_failure(&entry.provider, Some(class)).await;
                            last_error = Some(crate::error::SyscityError::ExternalService {
                                source: format!("Provider {} auth disabled: {}", entry.provider, e),
                                cause: None,
                            });
                            continue;
                        }

                        if class.should_rotate_key() {
                            let cooldown = self.cooldown_for_failure(&entry.provider, class).await;
                            match self
                                .rebuild_provider_with_rotated_key(&entry.provider, Some(cooldown))
                                .await
                            {
                                Ok(()) => {
                                    // Retry once with the new key
                                    let providers = self.providers.read().await;
                                    if let Some(provider) = providers.get(&entry.provider) {
                                        match provider.complete(request.clone()).await {
                                            Ok(response) => {
                                                self.record_success(
                                                    &entry.provider,
                                                    start.elapsed(),
                                                )
                                                .await;
                                                self.auth_profiles
                                                    .record_success(&entry.provider)
                                                    .await;
                                                if let Some(usage) = response.usage {
                                                    self.usage_tracker
                                                        .record(
                                                            &entry.provider,
                                                            usage,
                                                            &alias.model,
                                                        )
                                                        .await;
                                                }
                                                return Ok(response);
                                            }
                                            Err(e2) => {
                                                let class2 = FailureClass::from_error(&e2, None);
                                                error!(
                                                    "Provider {} failed after key rotation: {}",
                                                    entry.provider, e2
                                                );
                                                self.record_failure(&entry.provider, Some(class2))
                                                    .await;
                                                last_error = Some(e2);
                                            }
                                        }
                                    }
                                }
                                Err(rotate_err) => {
                                    error!(
                                        "Key rotation failed for provider {}: {}",
                                        entry.provider, rotate_err
                                    );
                                    self.record_failure(&entry.provider, Some(class)).await;
                                    last_error = Some(rotate_err);
                                }
                            }
                        } else {
                            self.record_failure(&entry.provider, Some(class)).await;
                            last_error = Some(crate::error::SyscityError::ExternalService {
                                source: format!("Provider {} failed: {}", entry.provider, e),
                                cause: None,
                            });
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

    /// Stream a completion through the router with fallback and key rotation.
    ///
    /// Mirrors `complete` but for streaming responses.  Key rotation and
    /// circuit-breaker logic are applied on stream *startup* failures only.
    pub async fn stream(
        &self,
        alias_or_model: &str,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionStream> {
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

        let mut request = CompletionRequest {
            model: Some(alias.model.clone()),
            messages,
            temperature: alias.temperature,
            max_tokens: alias.max_tokens,
            stream: true,
            tools,
            stop: None,
            extra: None,
            requires_vision: false,
            requires_tools: false,
            requires_reasoning: false,
            ..Default::default()
        };

        // Capability-aware routing
        let alias = self.resolve_alias_with_capabilities(&alias, &request).await;
        request.model = Some(alias.model.clone());

        let mut providers_to_try = self.get_provider_chain(&alias).await;
        for fallback in &request.fallback_models {
            let config = self.config.read().await;
            if let Some(fb_alias) = config.aliases.get(fallback).cloned() {
                drop(config);
                let fb_chain = self.get_provider_chain(&fb_alias).await;
                for entry in fb_chain {
                    if !providers_to_try
                        .iter()
                        .any(|e| e.provider == entry.provider && e.model == entry.model)
                    {
                        providers_to_try.push(entry);
                    }
                }
            } else {
                drop(config);
            }
        }

        let mut last_error = None;

        for entry in providers_to_try {
            if !entry.enabled {
                continue;
            }
            if self.is_circuit_open(&entry.provider).await {
                warn!("Circuit breaker open for provider: {}", entry.provider);
                continue;
            }

            let providers = self.providers.read().await;
            if let Some(provider) = providers.get(&entry.provider) {
                let start = std::time::Instant::now();

                match provider.stream(request.clone()).await {
                    Ok(stream) => {
                        self.record_success(&entry.provider, start.elapsed()).await;
                        self.auth_profiles.record_success(&entry.provider).await;
                        return Ok(stream);
                    }
                    Err(ref e) => {
                        let class = FailureClass::from_error(e, None);
                        warn!(
                            "Provider {} stream failed with {}: {}",
                            entry.provider,
                            class.description(),
                            e
                        );
                        drop(providers);

                        if class == FailureClass::ModelNotFound {
                            self.model_catalog
                                .suppress(&entry.provider, &entry.model)
                                .await;
                        }

                        if class.should_disable_key() {
                            let cooldown = self.cooldown_for_failure(&entry.provider, class).await;
                            if let Err(disable_err) = self
                                .rebuild_provider_with_rotated_key(&entry.provider, Some(cooldown))
                                .await
                            {
                                error!(
                                    "Key disable/rotation failed for provider {}: {}",
                                    entry.provider, disable_err
                                );
                            }
                            self.record_failure(&entry.provider, Some(class)).await;
                            last_error = Some(crate::error::SyscityError::ExternalService {
                                source: format!("Provider {} auth disabled: {}", entry.provider, e),
                                cause: None,
                            });
                            continue;
                        }

                        if class.should_rotate_key() {
                            let cooldown = self.cooldown_for_failure(&entry.provider, class).await;
                            match self
                                .rebuild_provider_with_rotated_key(&entry.provider, Some(cooldown))
                                .await
                            {
                                Ok(()) => {
                                    let providers = self.providers.read().await;
                                    if let Some(provider) = providers.get(&entry.provider) {
                                        match provider.stream(request.clone()).await {
                                            Ok(stream) => {
                                                self.record_success(
                                                    &entry.provider,
                                                    start.elapsed(),
                                                )
                                                .await;
                                                self.auth_profiles
                                                    .record_success(&entry.provider)
                                                    .await;
                                                return Ok(stream);
                                            }
                                            Err(e2) => {
                                                let class2 = FailureClass::from_error(&e2, None);
                                                error!(
                                                    "Provider {} stream failed after key \
                                                     rotation: {}",
                                                    entry.provider, e2
                                                );
                                                self.record_failure(&entry.provider, Some(class2))
                                                    .await;
                                                last_error = Some(e2);
                                            }
                                        }
                                    }
                                }
                                Err(rotate_err) => {
                                    error!(
                                        "Key rotation failed for provider {}: {}",
                                        entry.provider, rotate_err
                                    );
                                    self.record_failure(&entry.provider, Some(class)).await;
                                    last_error = Some(rotate_err);
                                }
                            }
                        } else {
                            self.record_failure(&entry.provider, Some(class)).await;
                            last_error = Some(crate::error::SyscityError::ExternalService {
                                source: format!("Provider {} failed: {}", entry.provider, e),
                                cause: None,
                            });
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

    /// Complete a request with cost-aware automatic model selection
    ///
    /// If cost-aware routing is enabled in config, this method classifies the
    /// task type from the messages and routes to the most cost-effective model
    /// alias for that task. Otherwise, it falls back to the default alias.
    pub async fn complete_auto(
        &self,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
    ) -> crate::Result<CompletionResponse> {
        let config = self.config.read().await;

        // Check if cost-aware routing is enabled
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
        let task_type = TaskClassifier::classify(&messages);
        info!("Task classified as: {:?}", task_type);

        let config = self.config.read().await;
        #[allow(clippy::expect_used)] // cost_aware presence checked by caller
        let cost_aware = config
            .cost_aware
            .as_ref()
            .expect("cost_aware config checked above");

        // Check budget limit
        if let Some(budget) = cost_aware.budget_limit_usd {
            let current_spend = cost_aware.daily_spend_usd;
            if current_spend >= budget {
                warn!(
                    "Daily budget exceeded: ${:.2} / ${:.2}. Falling back to cheapest model.",
                    current_spend, budget
                );
                // Find cheapest alias
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
                drop(config);
                return self.complete(&cheapest, messages, tools).await;
            }
        }

        // Find routing rule for this task type
        let rule = cost_aware
            .routing_rules
            .iter()
            .find(|r| r.task_type == task_type)
            .or_else(|| {
                cost_aware
                    .routing_rules
                    .iter()
                    .find(|r| r.task_type == TaskType::Unknown)
            });

        let alias_name = if let Some(rule) = rule {
            // Check token limit - if exceeded, use fallback (usually larger model)
            let estimated_tokens: u32 = messages.iter().map(|m| m.content.len() as u32 / 4).sum();

            if let Some(max_tokens) = rule.max_input_tokens {
                if estimated_tokens > max_tokens {
                    info!(
                        "Estimated tokens ({}) exceeds max for '{}' ({}), using fallback",
                        estimated_tokens, rule.preferred_alias, max_tokens
                    );
                    if let Some(ref fallback) = rule.fallback_alias {
                        fallback.clone()
                    } else {
                        rule.preferred_alias.clone()
                    }
                } else {
                    rule.preferred_alias.clone()
                }
            } else {
                rule.preferred_alias.clone()
            }
        } else {
            cost_aware.default_alias.clone()
        };
        drop(config);

        // Complete and track cost
        let alias_name_for_cost = alias_name.clone();
        let response = self.complete(&alias_name, messages, tools).await?;

        // Update spend if usage is available
        if let Some(ref usage) = response.usage {
            let mut config = self.config.write().await;
            if let Some(ref mut cost_aware) = config.cost_aware {
                if let Some(cost) = cost_aware.model_costs.get(&alias_name_for_cost) {
                    let estimated = cost.estimate(usage);
                    cost_aware.daily_spend_usd += estimated;
                    info!(
                        "Cost tracked: ${:.4} for '{}' (task: {:?})",
                        estimated, alias_name_for_cost, task_type
                    );
                }
            }
        }

        Ok(response)
    }

    /// Get current daily spend
    pub async fn get_daily_spend(&self) -> f64 {
        let config = self.config.read().await;
        if let Some(ref cost_aware) = config.cost_aware {
            cost_aware.daily_spend_usd
        } else {
            0.0
        }
    }

    /// Reset daily spend counter
    pub async fn reset_daily_spend(&self) {
        let mut config = self.config.write().await;
        if let Some(ref mut cost_aware) = config.cost_aware {
            cost_aware.daily_spend_usd = 0.0;
            info!("Daily spend counter reset");
        }
    }

    // ------------------------------------------------------------------
    // Usage snapshot enrichment with remote quota
    // ------------------------------------------------------------------

    /// Get a usage snapshot enriched with remote quota (if a fetcher is
    /// registered).
    pub async fn snapshot_with_quota(&self, provider: &str) -> Option<ProviderUsageSnapshot> {
        let mut snapshot = self.usage_tracker.snapshot(provider).await?;

        let fetchers = self.usage_fetchers.read().await;
        if let Some(fetcher) = fetchers.get(provider) {
            match fetcher.fetch().await {
                Ok(Some(quota)) => {
                    snapshot.quota = Some(quota);
                }
                Ok(None) => {
                    // No remote quota — try local budget fallback
                    drop(fetchers);
                    snapshot.quota = self.local_budget_quota(provider).await;
                }
                Err(e) => {
                    debug!("Failed to fetch remote quota for {}: {}", provider, e);
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
        let mut enriched = Vec::with_capacity(base_snapshots.len());

        for mut snapshot in base_snapshots {
            let provider = snapshot.provider.clone();
            let quota = {
                let fetchers = self.usage_fetchers.read().await;
                if let Some(fetcher) = fetchers.get(&provider) {
                    match fetcher.fetch().await {
                        Ok(Some(q)) => Some(q),
                        _ => self.local_budget_quota(&provider).await,
                    }
                } else {
                    drop(fetchers);
                    self.local_budget_quota(&provider).await
                }
            };
            snapshot.quota = quota;
            snapshot.last_updated = Utc::now();
            enriched.push(snapshot);
        }

        enriched
    }

    /// Build a local-budget quota when no remote fetcher is available.
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
    ///
    /// If the request requires vision/tools/reasoning and the resolved alias
    /// maps to a model that lacks those capabilities, search the catalog for
    /// the cheapest compatible model and switch to that alias.
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

        // No compatible model found — fall back to original alias
        alias.clone()
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

            // Adjust threshold based on failure class
            let effective_threshold = match class {
                Some(FailureClass::RateLimit) => threshold.saturating_sub(2).max(1),
                Some(FailureClass::Overloaded) => threshold.saturating_sub(1).max(1),
                _ => threshold,
            };

            if h.failures >= effective_threshold && h.state == CircuitState::Closed {
                warn!(
                    "Circuit breaker opened for provider: {} ({} failures, class={:?})",
                    provider, h.failures, class
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
            // First handle circuit-breaker state transitions (Open → HalfOpen).
            {
                let mut health = self.health.write().await;
                if let Some(h) = health.get_mut(&name) {
                    h.last_health_check = Some(chrono::Utc::now());

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

            // Send a lightweight real request to check liveness.
            let provider = {
                let providers = self.providers.read().await;
                providers.get(&name).cloned()
            };

            if let Some(provider) = provider {
                let request = crate::providers::CompletionRequest {
                    model: None,
                    messages: vec![crate::providers::Message::user("ping")],
                    temperature: Some(0.0),
                    max_tokens: Some(1),
                    stream: false,
                    tools: None,
                    stop: None,
                    extra: None,
                    ..Default::default()
                };

                let start = std::time::Instant::now();
                match provider.complete(request).await {
                    Ok(_) => self.record_success(&name, start.elapsed()).await,
                    Err(e) => {
                        debug!("Health probe failed for {}: {}", name, e);
                        self.record_failure(&name, None).await;
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
                let provider =
                    crate::providers::anthropic::AnthropicProvider::new(api_key.clone())?;
                let provider_arc = Arc::new(provider);

                // Register auth profile
                self.auth_profiles
                    .register_single_key("anthropic", api_key)
                    .await;

                // Store it for future use
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
            }
            .into());
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

    /// Resolve an alias to its actual model ID.
    pub async fn resolve_alias(&self, alias_name: &str) -> Option<String> {
        let config = self.config.read().await;
        config.aliases.get(alias_name).map(|a| a.model.clone())
    }

    /// List all available providers with their status
    pub async fn list_providers(&self) -> Vec<ProviderInfo> {
        let providers = self.providers.read().await;
        let health = self.health.read().await;
        let config = self.config.read().await;

        providers
            .keys()
            .map(|name| {
                let h = health.get(name).cloned().unwrap_or_default();
                let provider_config = config.providers.get(name).cloned();

                ProviderInfo {
                    name: name.clone(),
                    provider_type: provider_config
                        .as_ref()
                        .map(|c| format!("{:?}", c.provider_type))
                        .unwrap_or_default(),
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
            }
            .into())
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
            }
            .into())
        }
    }

    /// Add a new provider at runtime
    pub async fn add_provider(&self, name: &str, config: ProviderConfig) -> crate::Result<()> {
        info!("Adding new provider at runtime: {}", name);

        // Register auth profile
        let auth_config = config.derived_auth_profile_config();
        self.auth_profiles
            .register_from_config(name, &auth_config)
            .await;

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

    /// Add a pre-built provider instance at runtime (e.g. from a plugin).
    pub async fn add_provider_instance(
        &self,
        name: &str,
        provider: Arc<dyn crate::providers::Provider + Send + Sync>,
    ) -> crate::Result<()> {
        info!("Adding provider instance at runtime: {}", name);

        let mut providers = self.providers.write().await;
        providers.insert(name.to_string(), provider);
        drop(providers);

        let mut health = self.health.write().await;
        health.insert(name.to_string(), ProviderHealth::default());

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
            }
            .into());
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
            extra: None,
            ..Default::default()
        };

        let start = std::time::Instant::now();
        match provider.complete(request).await {
            Ok(_) => {
                self.record_success(name, start.elapsed()).await;
                Ok(true)
            }
            Err(_) => {
                self.record_failure(name, None).await;
                Ok(false)
            }
        }
    }

    /// Complete a request with a specific provider override (per-request
    /// override)
    pub async fn complete_with_provider(
        &self,
        provider_name: &str,
        model: Option<String>,
        messages: Vec<Message>,
        tools: Option<Vec<ToolDefinition>>,
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
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Provider {} circuit is open", provider_name),
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

        let start = std::time::Instant::now();
        match provider.complete(request.clone()).await {
            Ok(response) => {
                self.record_success(provider_name, start.elapsed()).await;
                self.auth_profiles.record_success(provider_name).await;
                if let Some(usage) = response.usage {
                    let model_name = model_id.as_deref().unwrap_or("unknown");
                    self.usage_tracker
                        .record(provider_name, usage, model_name)
                        .await;
                }
                Ok(response)
            }
            Err(ref e) => {
                let class = FailureClass::from_error(e, None);
                warn!("Provider {} failed with {}: {}", provider_name, class.description(), e);

                if class.should_disable_key() {
                    let cooldown = self.cooldown_for_failure(provider_name, class).await;
                    let _ = self
                        .rebuild_provider_with_rotated_key(provider_name, Some(cooldown))
                        .await;
                    self.record_failure(provider_name, Some(class)).await;
                    return Err(crate::error::SyscityError::ExternalService {
                        source: format!("Provider {} auth disabled: {}", provider_name, e),
                        cause: None,
                    });
                }

                if class.should_rotate_key() {
                    let cooldown = self.cooldown_for_failure(provider_name, class).await;
                    match self
                        .rebuild_provider_with_rotated_key(provider_name, Some(cooldown))
                        .await
                    {
                        Ok(()) => {
                            let providers = self.providers.read().await;
                            if let Some(provider) = providers.get(provider_name) {
                                match provider.complete(request).await {
                                    Ok(response) => {
                                        self.record_success(provider_name, start.elapsed()).await;
                                        self.auth_profiles.record_success(provider_name).await;
                                        if let Some(usage) = response.usage {
                                            let m = model_id.as_deref().unwrap_or("unknown");
                                            self.usage_tracker
                                                .record(provider_name, usage, m)
                                                .await;
                                        }
                                        Ok(response)
                                    }
                                    Err(e2) => {
                                        let class2 = FailureClass::from_error(&e2, None);
                                        error!(
                                            "Provider {} failed after key rotation: {}",
                                            provider_name, e2
                                        );
                                        self.record_failure(provider_name, Some(class2)).await;
                                        Err(e2)
                                    }
                                }
                            } else {
                                Err(crate::error::ConfigError::InvalidValue {
                                    key: "provider".to_string(),
                                    message: format!(
                                        "Provider '{}' not found after rotation",
                                        provider_name
                                    ),
                                }
                                .into())
                            }
                        }
                        Err(rotate_err) => {
                            error!(
                                "Key rotation failed for provider {}: {}",
                                provider_name, rotate_err
                            );
                            self.record_failure(provider_name, Some(class)).await;
                            Err(rotate_err)
                        }
                    }
                } else {
                    self.record_failure(provider_name, Some(class)).await;
                    Err(crate::error::SyscityError::ExternalService {
                        source: format!("Provider {} failed: {}", provider_name, e),
                        cause: None,
                    })
                }
            }
        }
    }

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
                message: format!("Unknown alias: {}", alias_name),
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

        let mut chains = self.fallback_chains.write().await;
        chains.insert(alias_name.to_string(), entries);

        // Also update config
        let mut config = self.config.write().await;
        config
            .fallback_chains
            .insert(alias_name.to_string(), provider_chain);

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
        // Check provider exists
        let providers = self.providers.read().await;
        if !providers.contains_key(provider_name) {
            return Err(crate::error::ConfigError::InvalidValue {
                key: "provider".to_string(),
                message: format!("Unknown provider: {}", provider_name),
            }
            .into());
        }
        drop(providers);

        // Rotate key
        match self.auth_profiles.rotate(provider_name, 60).await {
            Some(new_key) => {
                // Rebuild provider with new key
                self.rebuild_provider_with_rotated_key(provider_name, None)
                    .await?;
                info!("Manually rotated auth key for provider '{}'", provider_name);
                Ok(new_key)
            }
            None => Err(crate::error::SyscityError::ExternalService {
                source: format!(
                    "No available API keys for provider '{}' after rotation",
                    provider_name
                ),
                cause: None,
            }),
        }
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
    async fn complete(&self, request: CompletionRequest) -> crate::Result<CompletionResponse>;

    /// Stream a completion
    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> crate::Result<tokio::sync::mpsc::Receiver<crate::Result<CompletionResponse>>>;

    /// Health check
    async fn health_check(&self) -> crate::Result<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_config_has_no_aliases() {
        let config = ModelRouterConfig::default();
        assert!(config.aliases.is_empty());
        assert_eq!(config.default_model, "");
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
            api_key: "test-key".to_string(),
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
            api_key: "test-key".to_string(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        router.add_provider("p1", config).await.unwrap();

        // Default state is Closed (enabled)
        let providers = router.list_providers().await;
        assert!(providers[0].enabled);
        assert_eq!(providers[0].circuit_state, CircuitState::Closed);

        // Disable
        router.disable_provider("p1").await.unwrap();
        let providers = router.list_providers().await;
        assert!(!providers[0].enabled);
        assert_eq!(providers[0].circuit_state, CircuitState::Open);

        // Enable
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
    async fn provider_config_effective_key_prefers_auth_profile() {
        let mut config = ProviderConfig {
            provider_type: ProviderType::OpenAi,
            api_key: "single-key".to_string(),
            api_keys: vec!["multi-key".to_string()],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        // api_keys takes precedence over api_key
        assert_eq!(config.effective_key(), "multi-key");

        // auth_profile takes precedence over both
        config.auth_profile = Some(AuthProfileConfig {
            keys: vec![auth_profile::AuthKeyConfig {
                key: "profile-key".to_string(),
                label: "primary".to_string(),
            }],
            cooldown_secs: 60,
            max_failures: 3,
        });
        assert_eq!(config.effective_key(), "profile-key");
    }

    #[tokio::test]
    async fn circuit_state_default_is_closed() {
        let state = CircuitState::default();
        assert_eq!(state, CircuitState::Closed);
    }

    #[tokio::test]
    async fn get_provider_health_returns_info() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let config = ProviderConfig {
            provider_type: ProviderType::Anthropic,
            api_key: "test-key".to_string(),
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
            api_key: "key".to_string(),
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

        // Set chain
        router
            .set_fallback_chain("default", vec!["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        let chain = router.get_fallback_chain("default").await;
        assert_eq!(chain, vec!["a", "b"]);

        // Clear by setting empty
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

        // Switch back
        router.switch_default_model("default").await.unwrap();
        assert_eq!(router.get_default_model().await, "default");
    }

    #[tokio::test]
    async fn default_model_is_empty_by_default() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let default = router.get_default_model().await;
        assert_eq!(default, "");
    }

    #[test]
    fn task_classifier_detects_coding() {
        let msgs = vec![crate::providers::Message::user(
            "Write a function to sort an array in Python",
        )];
        assert_eq!(TaskClassifier::classify(&msgs), TaskType::Coding);
    }

    #[test]
    fn task_classifier_detects_summarization() {
        let msgs = vec![crate::providers::Message::user(
            "Summarize this article for me",
        )];
        assert_eq!(TaskClassifier::classify(&msgs), TaskType::Summarization);
    }

    #[test]
    fn task_classifier_detects_reasoning() {
        let msgs = vec![crate::providers::Message::user(
            "Explain why the sky is blue step by step",
        )];
        assert_eq!(TaskClassifier::classify(&msgs), TaskType::Reasoning);
    }

    #[test]
    fn task_classifier_defaults_to_chat() {
        let msgs = vec![crate::providers::Message::user("Hello, how are you today?")];
        assert_eq!(TaskClassifier::classify(&msgs), TaskType::Chat);
    }

    #[test]
    fn task_classifier_detects_classification() {
        let msgs = vec![crate::providers::Message::user(
            "Classify this text as positive or negative",
        )];
        assert_eq!(TaskClassifier::classify(&msgs), TaskType::Classification);
    }

    #[test]
    fn task_classifier_detects_translation() {
        let msgs = vec![crate::providers::Message::user("Translate this to French")];
        assert_eq!(TaskClassifier::classify(&msgs), TaskType::Translation);
    }

    #[test]
    fn task_classifier_detects_extraction() {
        let msgs = vec![crate::providers::Message::user(
            "Extract all email addresses from this text",
        )];
        assert_eq!(TaskClassifier::classify(&msgs), TaskType::Extraction);
    }

    #[test]
    fn model_cost_estimate() {
        let cost = ModelCost {
            input_cost_per_1k: 3.0,
            output_cost_per_1k: 15.0,
        };
        let usage = crate::providers::Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
        };
        let estimated = cost.estimate(&usage);
        assert!((estimated - 10.5).abs() < 0.001);
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

    #[test]
    fn cost_aware_budget_limit_none_by_default() {
        let config = CostAwareConfig::default();
        assert!(config.budget_limit_usd.is_none());
    }
}
