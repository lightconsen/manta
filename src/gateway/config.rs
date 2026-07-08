//! Gateway configuration types.
//!
//! All `*Config` structs that make up the top-level [`GatewayConfig`] tree
//! plus their `Default` impls and serde defaults. Extracted from
//! `gateway/mod.rs` so the control-plane file isn't dominated by data
//! definitions.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::agent::AgentConfig;
use crate::channels::ChannelType;
use crate::security::pairing::DmPolicy;
use crate::tools::mcp::McpSettings;

/// Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Host to bind to
    pub host: String,
    /// Port for gateway control plane (serves API + WebSocket + SPA)
    pub port: u16,
    /// Default agent configuration
    pub default_agent: AgentConfig,
    /// Channel configurations
    pub channels: HashMap<String, ChannelConfig>,
    /// Vector memory configuration
    #[serde(default)]
    pub vector_memory: VectorMemoryConfig,
    /// Plugin system configuration
    #[serde(default)]
    pub plugins: PluginConfig,
    /// Hot reload configuration
    #[serde(default)]
    pub hot_reload: HotReloadConfig,
    /// ACP (Agent Control Plane) configuration
    #[serde(default)]
    pub acp: AcpConfig,
    /// Cron scheduler configuration
    #[serde(default)]
    pub cron: CronConfig,
    /// Heartbeat scheduler configuration
    #[serde(default)]
    pub heartbeat: crate::heartbeat::HeartbeatConfig,
    /// Security configuration
    #[serde(default)]
    pub security: SecurityConfig,
    /// Storage adapter configuration
    #[serde(default)]
    pub storage: StorageConfig,
    /// LLM Provider configurations (provider name -> config)
    #[serde(default)]
    pub providers: HashMap<String, crate::model_router::ProviderConfig>,
    /// Default model name (e.g., "claude-3-sonnet-20240229", "qwen3.5-plus")
    #[serde(default = "default_model")]
    pub model: String,
    /// Model provider (e.g., "anthropic", "openai")
    #[serde(default = "default_model_provider")]
    pub model_provider: String,
    /// MCP server configurations (auto-connected on startup)
    #[serde(default)]
    pub mcp: McpSettings,
    /// Live spend and action-rate guard for LLM calls.
    #[serde(default)]
    pub cost_guard: CostGuardConfig,
    /// Workspace directory for file operations.
    /// All relative paths are resolved against this directory.
    /// When `workspace_only` is true, file operations are restricted to this
    /// directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<std::path::PathBuf>,
    /// When true, restrict file operations to `workspace_dir`.
    #[serde(default)]
    pub workspace_only: bool,
    /// Browser configuration (bridge, profiles, pool)
    #[cfg(feature = "browser")]
    #[serde(default)]
    pub browser: crate::config::BrowserConfig,
    /// Computer / desktop automation configuration
    #[serde(default)]
    pub computer: crate::config::ComputerConfig,
    /// Dream scheduler configuration for background memory consolidation
    #[serde(default)]
    pub dreaming: crate::config::MemoryDreamingConfig,
    /// Standing orders configuration (persistent background agent programs)
    #[serde(default)]
    pub standing_orders: crate::standing_orders::config::StandingOrderConfig,
    /// Capability set configuration (profile, scope, enabled sets)
    #[serde(default)]
    pub capabilities: crate::config::CapabilitiesConfig,
    /// Web search provider configuration.
    #[serde(default)]
    pub search: SearchConfig,
}

/// Default search provider name
fn default_search_provider() -> String {
    "duckduckgo".to_string()
}

/// Default provider API keys map
fn default_search_keys() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}

/// Default ordered list of search providers for fallback.
fn default_search_providers() -> Vec<String> {
    vec![default_search_provider()]
}

/// Web search provider configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Legacy single search provider name.
    /// Use `providers` for fallback ordering.
    #[serde(default = "default_search_provider")]
    pub provider: String,
    /// Ordered list of search providers to try.
    /// When empty, falls back to `[provider]`.
    #[serde(default = "default_search_providers")]
    pub providers: Vec<String>,
    /// Legacy single API key field.
    #[serde(default)]
    pub api_key: String,
    /// Per-provider API keys. Allows configuring multiple providers at once.
    /// The active provider uses the key from `keys[provider]` or falls back to `api_key`.
    #[serde(default = "default_search_keys")]
    pub keys: std::collections::HashMap<String, String>,
}

impl SearchConfig {
    /// Return the ordered list of provider names to try.
    /// Prefers `providers`; when empty, uses `[provider]`.
    pub fn provider_list(&self) -> Vec<String> {
        if self.providers.is_empty() {
            vec![self.provider.clone()]
        } else {
            self.providers.clone()
        }
    }

    /// Get the API key for a given provider name.
    /// Prefers `keys[provider]`, then the legacy `api_key`.
    pub fn api_key_for(&self, provider: &str) -> Option<String> {
        self.keys
            .get(provider)
            .cloned()
            .filter(|k| !k.is_empty())
            .or_else(|| self.api_key.clone().into())
            .filter(|k| !k.is_empty())
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            provider: default_search_provider(),
            providers: default_search_providers(),
            api_key: String::new(),
            keys: default_search_keys(),
        }
    }
}

fn default_model() -> String {
    "claude-3-sonnet-20240229".to_string()
}

fn default_model_provider() -> String {
    "anthropic".to_string()
}

/// Embedding provider type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum EmbeddingProviderType {
    /// OpenAI API (requires API key)
    #[default]
    OpenAi,
    /// Local GGUF model (direct loading, no external service)
    LocalGguf,
}

/// Vector memory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMemoryConfig {
    /// Enable vector memory / semantic search
    pub enabled: bool,
    /// Embedding provider type
    pub provider: EmbeddingProviderType,
    /// Embedding provider API key (e.g., OpenAI)
    pub embedding_api_key: Option<String>,
    /// Embedding model to use (for API providers)
    pub embedding_model: String,
    /// Embedding dimension
    pub embedding_dimension: usize,
    /// API base URL (for Azure, etc.)
    pub api_base_url: Option<String>,
    /// Local GGUF model path (for local-embeddings feature)
    pub local_model_path: Option<String>,
}

impl Default for VectorMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default to avoid blocking on model download
            provider: EmbeddingProviderType::LocalGguf,
            embedding_api_key: None,
            embedding_model: "text-embedding-3-small".to_string(),
            embedding_dimension: 1536,
            api_base_url: None,
            local_model_path: Some(
                "hf:unsloth/embedding-gemma-2b-GGUF/embedding-gemma-2b-Q4_K_M.gguf".to_string(),
            ),
        }
    }
}

/// Plugin system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Enable plugin system
    pub enabled: bool,
    /// Auto-load plugins on startup
    pub auto_load: bool,
    /// Plugin directory path (None = default)
    pub plugin_dir: Option<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_load: true,
            plugin_dir: None,
        }
    }
}

/// Hot reload configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotReloadConfig {
    /// Enable hot reload for configuration
    pub enabled: bool,
    /// Watch config files for changes
    pub watch_config: bool,
    /// Watch agent files for changes
    pub watch_agents: bool,
    /// Watch plugin files for changes
    pub watch_plugins: bool,
    /// Debounce duration in seconds
    pub debounce_seconds: u64,
}

impl Default for HotReloadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            watch_config: true,
            watch_agents: true,
            watch_plugins: true,
            debounce_seconds: 2,
        }
    }
}

/// ACP (Agent Control Plane) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfig {
    /// Enable subagent spawning
    pub enabled: bool,
    /// Maximum concurrent subagents
    pub max_subagents: usize,
    /// Default subagent timeout in seconds
    pub default_timeout_seconds: u64,
    /// Maximum iterations per ACP session execution
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

fn default_max_iterations() -> usize {
    100
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_subagents: 10,
            default_timeout_seconds: 300,
            max_iterations: default_max_iterations(),
        }
    }
}

/// Cron scheduler configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronConfig {
    /// Enable cron scheduler
    pub enabled: bool,
    /// Check interval in seconds
    pub check_interval_seconds: u64,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_seconds: 60,
        }
    }
}

/// Cost guard configuration — live spend and action-rate tracking.
///
/// Set `daily_limit_cents` and/or `hourly_action_limit` to non-zero values to
/// enable limits. Zero means unlimited (default).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CostGuardConfig {
    /// Maximum daily LLM spend in cents (0 = unlimited).
    /// Example: 500 = $5.00/day cap.
    #[serde(default)]
    pub daily_limit_cents: u64,
    /// Maximum provider calls per hour across all agents (0 = unlimited).
    #[serde(default)]
    pub hourly_action_limit: u64,
}

/// Credential source precedence for tokens, API keys, and passwords.
///
/// Controls which source wins when both environment variables and the
/// configuration file supply the same credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CredentialPrecedence {
    /// Environment variables take precedence over the config file.
    #[default]
    EnvFirst,
    /// The config file takes precedence over environment variables.
    ConfigFirst,
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Enable security features (auth, rate limiting, security headers)
    pub enabled: bool,
    /// Require authentication for API access
    pub auth_required: bool,
    /// Require pairing for new users
    pub pairing_required: bool,
    /// Authentication mode
    #[serde(default)]
    pub auth_mode: crate::gateway::protocol::AuthMode,
    /// Shared secret token for simple authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_token: Option<String>,
    /// Rate limiting configuration
    pub rate_limit: RateLimitConfig,
    /// Enable security headers
    pub security_headers: bool,
    /// OAuth2 configuration
    #[serde(default)]
    pub oauth: crate::gateway::auth::OAuthConfig,
    /// CORS configuration
    #[serde(default)]
    pub cors: crate::gateway::auth::CorsConfig,
    /// CSP configuration
    #[serde(default)]
    pub csp: crate::gateway::auth::CspConfig,
    /// Mention gating configuration
    #[serde(default)]
    pub mention_gating: crate::security::mention_gate::MentionGatingConfig,
    /// Allowed Tailscale tailnets (empty = any tailnet allowed when
    /// auth_mode=tailscale)
    #[serde(default)]
    pub allowed_tailnets: Vec<String>,
    /// Trusted proxy IPs for X-Forwarded-For header resolution
    #[serde(default)]
    pub trusted_proxies: Vec<std::net::IpAddr>,
    /// Tailscale whois cache TTL in seconds (default 300)
    #[serde(default = "default_tailscale_ttl")]
    pub tailscale_auth_ttl_secs: u64,
    /// Trusted proxy authentication configuration.
    #[serde(default)]
    pub trusted_proxy: crate::security::trusted_proxy::TrustedProxyConfig,
    /// Credential source precedence for tokens, API keys, and passwords.
    #[serde(default)]
    pub credential_precedence: CredentialPrecedence,
}

fn default_tailscale_ttl() -> u64 {
    300
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting
    pub enabled: bool,
    /// Maximum requests per window (legacy token bucket)
    pub capacity: u32,
    /// Refill rate (tokens per second) (legacy token bucket)
    pub refill_rate: f64,
    /// Use multi-tier sliding window rate limiting instead of token bucket
    #[serde(default)]
    pub multi_tier: bool,
    /// Global tier: overall API rate limit
    #[serde(default)]
    pub global: TierConfig,
    /// Per-authenticated-user rate limit
    #[serde(default)]
    pub per_user: TierConfig,
    /// Per-IP rate limit (for anonymous requests)
    #[serde(default)]
    pub per_ip: TierConfig,
    /// Per-endpoint rate limit
    #[serde(default)]
    pub per_endpoint: TierConfig,
    /// Shared-secret authentication scope rate limit.
    #[serde(default)]
    pub shared_secret: TierConfig,
    /// Device-token authentication scope rate limit.
    #[serde(default)]
    pub device_token: TierConfig,
    /// Webhook/hook authentication scope rate limit.
    #[serde(default)]
    pub hook_auth: TierConfig,
    /// Control-plane write operation rate limit.
    #[serde(default)]
    pub control_plane_write: TierConfig,
    /// Lockout configuration for repeated failures.
    #[serde(default)]
    pub lockout: crate::security::sliding_window::LockoutConfig,
    /// Skip rate limiting for loopback addresses.
    #[serde(default)]
    pub loopback_exempt: bool,
}

/// Single tier configuration for multi-tier rate limiting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierConfig {
    /// Enable this tier
    pub enabled: bool,
    /// Maximum requests per window
    pub capacity: u32,
    /// Window size in seconds
    pub window_secs: u64,
}

impl Default for TierConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            capacity: 100,
            window_secs: 60,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auth_required: false,
            pairing_required: false,
            auth_mode: crate::gateway::protocol::AuthMode::None,
            shared_token: None,
            rate_limit: RateLimitConfig::default(),
            security_headers: true,
            oauth: crate::gateway::auth::OAuthConfig::default(),
            cors: crate::gateway::auth::CorsConfig::default(),
            csp: crate::gateway::auth::CspConfig::default(),
            mention_gating: crate::security::mention_gate::MentionGatingConfig::default(),
            allowed_tailnets: Vec::new(),
            trusted_proxies: Vec::new(),
            tailscale_auth_ttl_secs: 300,
            trusted_proxy: crate::security::trusted_proxy::TrustedProxyConfig::default(),
            credential_precedence: CredentialPrecedence::default(),
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            capacity: 100,
            refill_rate: 10.0,
            multi_tier: true,
            global: TierConfig {
                enabled: true,
                capacity: 1000,
                window_secs: 60,
            },
            per_user: TierConfig {
                enabled: true,
                capacity: 100,
                window_secs: 60,
            },
            per_ip: TierConfig {
                enabled: true,
                capacity: 30,
                window_secs: 60,
            },
            per_endpoint: TierConfig {
                enabled: false,
                capacity: 50,
                window_secs: 60,
            },
            shared_secret: TierConfig {
                enabled: true,
                capacity: 200,
                window_secs: 60,
            },
            device_token: TierConfig {
                enabled: true,
                capacity: 60,
                window_secs: 60,
            },
            hook_auth: TierConfig {
                enabled: true,
                capacity: 300,
                window_secs: 60,
            },
            control_plane_write: TierConfig {
                enabled: true,
                capacity: 20,
                window_secs: 60,
            },
            lockout: crate::security::sliding_window::LockoutConfig::default(),
            loopback_exempt: true,
        }
    }
}

/// Storage adapter configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Storage type: "memory", "file", "sqlite"
    pub storage_type: String,
    /// Base path for file/SQLite storage
    pub base_path: Option<String>,
    /// SQLite database URL (if using sqlite)
    pub database_url: Option<String>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage_type: "sqlite".to_string(),
            base_path: None,
            database_url: None,
        }
    }
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 18080,
            default_agent: AgentConfig::default(),
            channels: HashMap::new(),
            vector_memory: VectorMemoryConfig::default(),
            plugins: PluginConfig::default(),
            hot_reload: HotReloadConfig::default(),
            acp: AcpConfig::default(),
            cron: CronConfig::default(),
            heartbeat: crate::heartbeat::HeartbeatConfig::default(),
            security: SecurityConfig::default(),
            storage: StorageConfig::default(),
            providers: HashMap::new(),
            model: default_model(),
            model_provider: default_model_provider(),
            mcp: McpSettings::default(),
            cost_guard: CostGuardConfig::default(),
            workspace_dir: None,
            workspace_only: false,
            #[cfg(feature = "browser")]
            browser: crate::config::BrowserConfig::default(),
            computer: crate::config::ComputerConfig::default(),
            dreaming: crate::config::MemoryDreamingConfig::default(),
            standing_orders: crate::standing_orders::config::StandingOrderConfig::default(),
            capabilities: crate::config::CapabilitiesConfig::default(),
            search: SearchConfig::default(),
        }
    }
}

/// Channel-specific configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Channel type
    pub channel_type: ChannelType,
    /// Whether channel is enabled
    pub enabled: bool,
    /// Channel-specific credentials/tokens
    pub credentials: HashMap<String, String>,
    /// DM policy: open, pairing, or allowlist
    #[serde(default)]
    pub dm_policy: DmPolicy,
    /// Require explicit mention in group chats (ignored for DMs)
    #[serde(default)]
    pub require_mention: bool,
    /// Allowlist of users/numbers (for allowlist policy)
    #[serde(default)]
    pub allow_from: Vec<String>,
    /// Blocklist of users/numbers
    #[serde(default)]
    pub block_from: Vec<String>,
    /// Agent ID to route to (None = default)
    pub agent_id: Option<String>,
}

impl ChannelConfig {
    /// Create a new channel config with open policy (default).
    pub fn new(channel_type: ChannelType) -> Self {
        Self {
            channel_type,
            enabled: true,
            credentials: HashMap::new(),
            dm_policy: DmPolicy::Open,
            require_mention: false,
            allow_from: Vec::new(),
            block_from: Vec::new(),
            agent_id: None,
        }
    }

    /// Set the DM policy.
    pub fn with_dm_policy(mut self, policy: DmPolicy) -> Self {
        self.dm_policy = policy;
        self
    }

    /// Set the allowlist.
    pub fn with_allow_from(mut self, allow_from: Vec<String>) -> Self {
        self.allow_from = allow_from;
        self
    }

    /// Check if a user is in the allowlist.
    pub fn is_in_allowlist(&self, user_id: &str) -> bool {
        self.allow_from.iter().any(|a| a == user_id)
    }

    /// Check if a user is blocked.
    pub fn is_blocked(&self, user_id: &str) -> bool {
        self.block_from.iter().any(|b| b == user_id)
    }
}

// ── Hot-reload snapshot/diff ─────────────────────────────────────────────────

/// Snapshot of hot-reloadable configuration values used to compute diffs
/// across reloads.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSnapshot {
    /// Timestamp when the snapshot was taken
    pub timestamp: String,
    /// The hot-reloadable field values keyed by dotted path (e.g.
    /// "providers.openai.base_url")
    pub fields: HashMap<String, serde_json::Value>,
}

/// A single configuration field change detected during hot reload.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigChange {
    /// Dotted path of the changed field (e.g. "model", "providers.openai")
    pub path: String,
    /// Previous value (absent for newly added fields)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<serde_json::Value>,
    /// New value (absent for removed fields)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<serde_json::Value>,
}

impl GatewayConfig {
    /// Capture a snapshot of all hot-reloadable fields.
    pub fn snapshot(&self) -> ConfigSnapshot {
        let mut fields = HashMap::new();

        let json = serde_json::to_value(self).unwrap_or_default();
        let obj = json.as_object().cloned().unwrap_or_default();

        // Only capture fields that are actually hot-reloadable
        let reloadable_keys = [
            "security",
            "providers",
            "mcp",
            "hot_reload",
            "cost_guard",
            "capabilities",
            "computer",
            "workspace_dir",
            "workspace_only",
            "model",
            "model_provider",
            "dreaming",
            "standing_orders",
            "cron",
            "browser",
        ];

        for key in &reloadable_keys {
            if let Some(val) = obj.get(*key) {
                if !val.is_null() {
                    fields.insert(key.to_string(), val.clone());
                }
            }
        }

        ConfigSnapshot {
            timestamp: chrono::Utc::now().to_rfc3339(),
            fields,
        }
    }

    /// Compute the list of field-level changes between a previous snapshot
    /// and the current configuration.
    pub fn diff_since(&self, old: &ConfigSnapshot) -> Vec<ConfigChange> {
        let current = self.snapshot();
        let mut changes = Vec::new();

        let all_keys: std::collections::BTreeSet<&String> =
            old.fields.keys().chain(current.fields.keys()).collect();

        for key in all_keys {
            let old_val = old.fields.get(key);
            let new_val = current.fields.get(key);

            match (old_val, new_val) {
                (Some(a), Some(b)) if a == b => continue,
                (Some(_), Some(b)) => {
                    changes.push(ConfigChange {
                        path: key.clone(),
                        old_value: old_val.cloned(),
                        new_value: Some(b.clone()),
                    });
                }
                (Some(a), None) => {
                    changes.push(ConfigChange {
                        path: key.clone(),
                        old_value: Some(a.clone()),
                        new_value: None,
                    });
                }
                (None, Some(b)) => {
                    changes.push(ConfigChange {
                        path: key.clone(),
                        old_value: None,
                        new_value: Some(b.clone()),
                    });
                }
                (None, None) => {}
            }
        }

        changes
    }
}
