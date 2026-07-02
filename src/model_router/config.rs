//! Configuration types for the Model Router
//!
//! Contains all configuration-related types extracted from the original
//! monolithic `mod.rs`: provider configs, routing rules, cost-aware routing,
//! circuit breaker state, health tracking, and built-in provider presets.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model_router::auth_profile::{AuthKeyConfig, AuthProfileConfig};

// ------------------------------------------------------------------
// ModelAlias
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// OAuthConfig
// ------------------------------------------------------------------

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
    /// OAuth2 client secret (required by some providers for token exchange)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Local redirect callback port (default: 18081)
    #[serde(default = "default_redirect_port")]
    pub redirect_port: u16,
}

fn default_redirect_port() -> u16 {
    18081
}

// ------------------------------------------------------------------
// ProviderType
// ------------------------------------------------------------------

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
            ProviderType::Custom { name } => write!(f, "{name}"),
        }
    }
}

// ------------------------------------------------------------------
// ProviderConfig
// ------------------------------------------------------------------

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

    /// Build an AuthProfileConfig from this config if one is not explicitly set.
    pub fn derived_auth_profile_config(&self) -> AuthProfileConfig {
        if let Some(ref profile) = self.auth_profile {
            return profile.clone();
        }
        let mut keys = Vec::new();
        if !self.api_key.is_empty() {
            keys.push(AuthKeyConfig {
                key: self.api_key.clone(),
                label: "primary".to_string(),
            });
        }
        for (i, key) in self.api_keys.iter().enumerate() {
            if i == 0 && key == &self.api_key {
                continue; // avoid duplicate
            }
            keys.push(AuthKeyConfig {
                key: key.clone(),
                label: format!("key-{i}"),
            });
        }
        AuthProfileConfig {
            keys,
            cooldown_secs: 60,
            max_failures: 3,
        }
    }
}

// ------------------------------------------------------------------
// ModelCost
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// TaskType
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// TaskRoutingRule
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// CostAwareConfig
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// ProviderPreset / provider_presets()
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// FallbackEntry
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// ModelRouterConfig
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// CircuitState
// ------------------------------------------------------------------

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CircuitState {
    #[default]
    Closed, // Normal operation
    Open,     // Failing, reject requests
    HalfOpen, // Testing if recovered
}

// ------------------------------------------------------------------
// ProviderHealth
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// ProviderInfo / ProviderHealthInfo
// ------------------------------------------------------------------

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

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circuit_state_default_is_closed() {
        let state = CircuitState::default();
        assert_eq!(state, CircuitState::Closed);
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
            keys: vec![AuthKeyConfig {
                key: "profile-key".to_string(),
                label: "primary".to_string(),
            }],
            cooldown_secs: 60,
            max_failures: 3,
        });
        assert_eq!(config.effective_key(), "profile-key");
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

    #[test]
    fn cost_aware_config_default_routing_rules() {
        let config = CostAwareConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.model_costs.len(), 3);
        assert_eq!(config.routing_rules.len(), 9);
        assert_eq!(config.default_alias, "default");
    }
}
