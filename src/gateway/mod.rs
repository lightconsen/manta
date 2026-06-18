//! Gateway Control Plane
//!
//! The Gateway is the control plane for Syscity, managing:
//! - Multi-channel message routing (WhatsApp, Telegram, Feishu, etc.)
//! - Session management and routing to agents
//! - Agent spawning and lifecycle management
//! - WebSocket/HTTP API for channel adapters
//! - Authentication and security policies

// Transitional: management REST handlers are no longer routed (protocol.md v1.0
// Phase 3) but kept in source for reference during the migration window.
// They will be fully removed in Phase 5 cleanup.

use axum::{
    middleware::{from_fn, from_fn_with_state},
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::session_store::AppendMessageParams;
use crate::agent::{Agent, AgentConfig};
use crate::channels::snapshot::healthy_snapshot;
use crate::channels::{Channel, ChannelExtension, ChannelType};
use crate::config::hot_reload::{ConfigFileType, HotReloadManager};
use crate::inbound::*;
use crate::security::pairing::DmPolicy;
use crate::tools::approval::ApprovalQueue;
use crate::tools::delegate_tool::AgentResolver;
use crate::tools::mcp::{McpManager, McpSettings, McpToolWrapper};
use crate::tools::ToolRegistry;
use async_trait::async_trait;

#[cfg(test)]
use crate::model_router::ModelRouter;
#[cfg(test)]
use crate::plugins::PluginManager;
#[cfg(test)]
use crate::canvas::CanvasManager;

pub mod auth;
pub mod command_provider;
pub mod commands;
pub mod handlers;
pub mod hooks;
pub mod init;
pub mod middleware;
pub mod protocol;
pub mod rate_limit;
pub mod send_policy;
pub mod state;
pub use state::*;

pub mod webhooks;
pub mod ws;
use handlers::*;

/// Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Host to bind to
    pub host: String,
    /// Port for gateway control plane (serves API + WebSocket + SPA)
    pub port: u16,
    /// Enable Tailscale remote access
    pub tailscale_enabled: bool,
    /// Tailscale funnel domain (if using)
    pub tailscale_domain: Option<String>,
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
    /// When `workspace_only` is true, file operations are restricted to this directory.
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
    /// Device subsystem configuration.
    #[serde(default)]
    pub device: DeviceConfig,
    /// Perception fusion layer configuration.
    #[serde(default)]
    pub perception: PerceptionConfig,
}

/// A single device driver entry in the configuration.
///
/// Each entry specifies the driver `kind` (e.g. `"mock"`, `"serial"`) and
/// optional JSON `params` passed to the driver's constructor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDriverEntry {
    /// Driver type name, e.g. `"mock"`.
    pub kind: String,
    /// Arbitrary JSON parameters for the driver constructor.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Device subsystem configuration for the gateway.
///
/// Controls whether the device subsystem is active and how drivers are
/// discovered and managed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// Enable the device subsystem. When `false`, all device driver
    /// probing and connection is skipped.
    #[serde(default = "default_device_enabled")]
    pub enabled: bool,
    /// List of device drivers to construct from configuration.
    #[serde(default)]
    pub drivers: Vec<DeviceDriverEntry>,
    /// Health check loop configuration.
    #[serde(default)]
    pub health_check: crate::device::HealthCheckConfig,
    /// Hot-plug detection loop configuration.
    #[serde(default)]
    pub hot_plug: crate::device::HotPlugConfig,
    /// OS device bridge configuration.
    #[serde(default)]
    pub os_bridge: crate::device::os_bridge::OsBridgeConfig,
    /// Control lane configuration (optional high-priority safety loop).
    #[serde(default)]
    pub control: crate::device::ControlConfig,
    /// Optional directory to scan for native plugin shared libraries
    /// (`.so`, `.dylib`, `.dll`) at startup.  Each plugin must export the
    /// `syscity_driver_*` C ABI functions.
    #[cfg(feature = "native-plugins")]
    #[serde(default)]
    pub native_plugins_dir: Option<std::path::PathBuf>,
}

fn default_device_enabled() -> bool {
    true
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            drivers: Vec::new(),
            health_check: crate::device::HealthCheckConfig::default(),
            hot_plug: crate::device::HotPlugConfig::default(),
            os_bridge: crate::device::os_bridge::OsBridgeConfig::default(),
            control: crate::device::ControlConfig::default(),
            #[cfg(feature = "native-plugins")]
            native_plugins_dir: None,
        }
    }
}

/// Backend selection for the perception summary engine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummarizerKind {
    /// Rule-based, zero-LLM template summarizer (default, free).
    #[default]
    Template,
    /// Small local GGUF model via llama-cpp-2 (requires `local-summarizer`
    /// feature).
    Local,
    /// Agent's existing LLM provider (billed like a normal model call).
    Llm,
}

/// Perception fusion layer configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionConfig {
    /// Enable the perception fusion layer. When false, no sources are polled.
    #[serde(default)]
    pub enabled: bool,
    /// Interval in seconds for the background poll loop. 0 = disable auto-poll.
    #[serde(default)]
    pub poll_interval_secs: u64,
    /// Maximum number of observations to retain in the scene graph history.
    #[serde(default = "default_scene_history")]
    pub scene_history: usize,
    /// Aggregation window in seconds for temporal fusion.
    #[serde(default = "default_aggregation_window")]
    pub aggregation_window_secs: u64,
    /// Audio input source name ("microphone" or "system_output").
    #[serde(default = "default_audio_source")]
    pub audio_source: String,
    /// Microphone sample rate in Hz (default 16_000).
    #[serde(default = "default_audio_sample_rate")]
    pub audio_sample_rate: u32,
    /// Silence threshold in dB for voice activity detection (default -40.0).
    #[serde(default = "default_silence_threshold_db")]
    pub silence_threshold_db: f32,
    /// Enable microphone as a perception source.
    #[serde(default)]
    pub enable_microphone: bool,
    /// Persistence backend: `"none"` (default, in-memory only) or `"jsonl"`
    /// (file-backed JSONL day files for cross-restart history).
    #[serde(default = "default_persistence_backend")]
    pub persistence_backend: String,
    /// Root directory for the persistence backend. When `None`, falls back
    /// to a sensible default (`{temp}/syscity-perception-jsonl`).
    #[serde(default)]
    pub persistence_dir: Option<String>,
    /// Days of observation history to retain. `0` disables pruning.
    /// The prune task runs every 6 hours.
    #[serde(default = "default_persistence_retention_days")]
    pub persistence_retention_days: u64,
    /// Master switch for the periodic summary refresh. When `false`, the
    /// background task that populates `Snapshot::summary` is never spawned
    /// even if a summarizer backend is configured. On-demand
    /// `adapter.summarize()` calls still work. Default: `false`.
    #[serde(default)]
    pub enable_summary: bool,
    /// Backend when `enable_summary` is `true`. Default: `Template`.
    #[serde(default)]
    pub summarizer_kind: SummarizerKind,
    /// Refresh interval in seconds for the summary background task.
    /// When `None` (or absent in config), defaults to 60 seconds.
    #[serde(default)]
    pub summary_refresh_secs: Option<u64>,
}

fn default_scene_history() -> usize {
    1000
}

fn default_aggregation_window() -> u64 {
    5
}

fn default_audio_source() -> String {
    "microphone".to_string()
}

fn default_audio_sample_rate() -> u32 {
    16_000
}

fn default_silence_threshold_db() -> f32 {
    -40.0
}

fn default_persistence_backend() -> String {
    "none".to_string()
}

fn default_persistence_retention_days() -> u64 {
    7
}

impl Default for PerceptionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            poll_interval_secs: 0,
            scene_history: 1000,
            aggregation_window_secs: 5,
            audio_source: "microphone".to_string(),
            audio_sample_rate: 16_000,
            silence_threshold_db: -40.0,
            enable_microphone: false,
            persistence_backend: "none".to_string(),
            persistence_dir: None,
            persistence_retention_days: 7,
            enable_summary: false,
            summarizer_kind: SummarizerKind::default(),
            summary_refresh_secs: None,
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
    50
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
    /// Allowed Tailscale tailnets (empty = any tailnet allowed when auth_mode=tailscale)
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
            tailscale_enabled: false,
            tailscale_domain: None,
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
            device: DeviceConfig::default(),
            perception: PerceptionConfig::default(),
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

impl GatewayState {
    /// Centralized access check for incoming messages.
    ///
    /// Returns `Ok(())` if the message is allowed, or `Err(reason)` if it
    /// should be dropped.
    pub async fn check_incoming_access(
        &self,
        channel: &str,
        user_id: &str,
        content: &str,
        mention: &crate::channels::MentionState,
    ) -> Result<(), String> {
        use crate::security::runtime_audit::AuditEventType;

        let channel_config = {
            let config = self.config.read().await;
            config.channels.get(channel).cloned()
        };

        if let Some(ref ch_cfg) = channel_config {
            // 1. Blocklist check
            if ch_cfg.is_blocked(user_id) {
                let reason = format!("User {} is blocked on channel {}", user_id, channel);
                self.auth.audit_log
                    .log(AuditEventType::AccessCheck, user_id, channel, false, &reason, None)
                    .await;
                return Err(reason);
            }

            // 2. DM Policy check
            use crate::security::pairing::DmPolicy;
            match ch_cfg.dm_policy {
                DmPolicy::Open => {}
                DmPolicy::Pairing => {
                    if !self.auth.pairing_store.is_authorized(channel, user_id).await {
                        // Create pairing request silently and drop message
                        let _ = self.auth.pairing_store
                            .request_access(channel, user_id, None)
                            .await;
                        let reason = format!(
                            "User {} not authorized on channel {} (pairing required)",
                            user_id, channel
                        );
                        self.auth.audit_log
                            .log(
                                AuditEventType::PairingRequest,
                                user_id,
                                channel,
                                false,
                                &reason,
                                None,
                            )
                            .await;
                        return Err(reason);
                    }
                }
                DmPolicy::Allowlist => {
                    if !ch_cfg.is_in_allowlist(user_id)
                        && !self.auth.pairing_store.is_authorized(channel, user_id).await
                    {
                        let reason =
                            format!("User {} not in allowlist for channel {}", user_id, channel);
                        self.auth.audit_log
                            .log(
                                AuditEventType::AccessCheck,
                                user_id,
                                channel,
                                false,
                                &reason,
                                None,
                            )
                            .await;
                        return Err(reason);
                    }
                }
            }

            // 3. Mention gating (require_mention + MentionGate)
            if !mention.should_process(ch_cfg.require_mention) {
                let reason = format!(
                    "Message from {} on channel {} ignored (mention required in groups)",
                    user_id, channel
                );
                self.auth.audit_log
                    .log(AuditEventType::AccessCheck, user_id, channel, false, &reason, None)
                    .await;
                return Err(reason);
            }

            // 3b. MentionGate policy check
            if matches!(mention, crate::channels::MentionState::Mentioned) {
                let mention_allowed = self.auth.mention_gate.check(channel, "*").await;
                if !mention_allowed {
                    let reason = format!(
                        "Mention gate blocked message on channel {} (policy: {})",
                        channel,
                        self.auth.mention_gate.policy().await
                    );
                    self.auth.audit_log
                        .log(AuditEventType::AccessCheck, user_id, channel, false, &reason, None)
                        .await;
                    return Err(reason);
                }
            }
        }

        // 4. Command gate check
        let decision = self.auth.command_gate.check(user_id, content);
        if !decision.is_allowed() {
            let reason = match decision {
                crate::tools::command_gate::AccessDecision::Denied { reason, .. } => reason,
                _ => "Unknown denial reason".to_string(),
            };
            let msg = format!("Command gate denied for user {}: {}", user_id, reason);
            self.auth.audit_log
                .log(
                    AuditEventType::CommandGate,
                    user_id,
                    channel,
                    false,
                    &msg,
                    Some(serde_json::json!({"reason": reason})),
                )
                .await;
            return Err(msg);
        }

        // Log successful access
        self.auth.audit_log
            .log(AuditEventType::AccessCheck, user_id, channel, true, "Access allowed", None)
            .await;

        Ok(())
    }
}

/// A buffered message awaiting batch processing (FollowUp / Collect modes).
#[derive(Debug, Clone)]
pub struct BufferedMessage {
    pub content: String,
    pub user_id: String,
    pub channel: String,
}

/// Handle to a running agent
#[derive(Clone)]
pub struct AgentHandle {
    /// Agent ID
    pub id: String,
    /// Agent configuration
    pub config: AgentConfig,
    /// Fire-and-forget command channel (ProcessMessage, Cancel, UpdateConfig, Shutdown)
    pub tx: mpsc::Sender<AgentCommand>,
    /// Request/response query channel (introspection + skill invocations)
    pub query_tx: mpsc::Sender<AgentQuery>,
    /// Whether agent is currently processing
    pub busy: bool,
    /// Reference to the agent for ACP orchestration
    pub agent: Arc<Agent>,
}

/// Wraps the Gateway agent map for [`AgentResolver`] lookups.
///
/// When [`DelegateTool`] receives a `target_agent` field, this resolver
/// looks up the corresponding running agent from the Gateway's agent pool
/// so the child task is routed to the specialised agent.
struct GatewayAgentResolver {
    agents: Arc<RwLock<HashMap<String, AgentHandle>>>,
}

#[async_trait]
impl AgentResolver for GatewayAgentResolver {
    async fn resolve(&self, name: &str) -> Option<Arc<Agent>> {
        let agents = self.agents.read().await;
        agents.get(name).map(|h| h.agent.clone())
    }
}

/// Commands sent to agents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentCommand {
    /// Process a message
    ProcessMessage {
        session_id: String,
        message: String,
        user_id: String,
        channel: String,
        /// Optional model override (e.g. from OpenAI-compatible API header/query).
        #[serde(skip_serializing_if = "Option::is_none")]
        model_override: Option<String>,
    },
    /// Cancel current operation
    Cancel,
    /// Update configuration
    UpdateConfig(AgentConfig),
    /// Shutdown agent
    Shutdown,
}

/// Query messages that require a typed response via oneshot channel.
/// Kept separate from AgentCommand because oneshot::Sender<T> cannot implement
/// the Clone/Serialize/Deserialize derives that AgentCommand carries.
#[allow(clippy::type_complexity)]
pub enum AgentQuery {
    /// Return all thread summaries for this agent's session store.
    GetThreadSummaries {
        response_tx: tokio::sync::oneshot::Sender<Vec<(String, String, usize, String)>>,
    },
    /// Return the turns for a specific conversation/thread.
    GetThreadTurns {
        conv_id: String,
        response_tx: tokio::sync::oneshot::Sender<Option<Vec<(usize, String, String, String)>>>,
    },
    /// Undo the last turn in a conversation.
    UndoLastTurn {
        conv_id: String,
        response_tx: tokio::sync::oneshot::Sender<bool>,
    },
    /// Redo the most recently undone turn in a conversation.
    RedoLastTurn {
        conv_id: String,
        response_tx: tokio::sync::oneshot::Sender<bool>,
    },
    /// Process a message as a skill invocation (request/response pattern).
    RunSkill {
        session_id: String,
        message: String,
        user_id: String,
        /// Trust level of the invoking skill — constrains which tools are available.
        skill_trust: crate::tools::SkillTrust,
        response_tx:
            tokio::sync::oneshot::Sender<crate::error::Result<crate::channels::OutgoingMessage>>,
    },
}

/// Events broadcast by gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GatewayEvent {
    /// Message received from channel
    MessageReceived {
        channel: String,
        user_id: String,
        content: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    /// Agent response ready
    AgentResponse {
        session_id: String,
        agent_id: String,
        content: String,
        channel: String,
        /// Channel-specific conversation ID for routing responses
        conversation_id: String,
        /// Token usage (prompt, completion, total) if available
        usage: Option<crate::providers::Usage>,
    },
    /// Agent status changed
    AgentStatus {
        agent_id: String,
        status: AgentStatus,
    },
    /// Channel connected/disconnected
    ChannelStatus { channel: String, connected: bool },
    /// Tool execution started
    ToolCalling {
        session_id: String,
        agent_id: String,
        tool_name: String,
        arguments: String,
    },
    /// Tool execution completed
    ToolResult {
        session_id: String,
        agent_id: String,
        tool_name: String,
        result: String,
        data: Option<serde_json::Value>,
    },
    /// High-risk tool call is waiting for human approval
    ApprovalRequired {
        approval_id: String,
        tool_name: String,
        requested_by: String,
        risk_level: crate::tools::approval::RiskLevel,
        message: String,
    },
    /// Device pairing request initiated
    DevicePairRequested {
        device_id: String,
        code: String,
        display_name: Option<String>,
    },
    /// New session auto-created during chat.send
    SessionCreated {
        session_id: String,
        agent_id: String,
        user_id: String,
    },
    /// Session display name was auto-generated or updated
    SessionRenamed { session_id: String, name: String },
    /// ACP subagent spawned
    AcpSpawned {
        session_id: String,
        subagent_id: String,
        parent_id: String,
        mode: String,
        thread_id: String,
    },
    /// ACP subagent completed / terminated / crashed
    AcpCompleted {
        session_id: String,
        subagent_id: String,
        status: String,
    },
    /// ACP subagent runtime state changed (pause/resume/step/cancel)
    AcpStatusChanged {
        session_id: String,
        runtime_state: String,
    },
    /// ACP crashed subagent recovered
    AcpRecovered {
        session_id: String,
        old_subagent_id: String,
        new_subagent_id: String,
        crash_count: u32,
    },
    /// ACP thread active subagent switched
    AcpThreadSwitched {
        thread_id: String,
        active_subagent: Option<String>,
    },
    /// MCP server connected
    McpConnected {
        server_id: String,
        tools: usize,
        prompts: usize,
        resources: usize,
    },
    /// MCP server disconnected or marked unhealthy
    McpDisconnected { server_id: String, reason: String },
    /// MCP server recovered after automatic reconnect
    McpRecovered { server_id: String, attempt: u32 },
    /// MCP subscribed resource changed
    McpResourceChanged { server_id: String, uri: String },
    /// Self-repair action taken (agent or channel restarted)
    RepairAction {
        /// "agent" or "channel"
        kind: String,
        target_id: String,
        description: String,
        restart_count: u32,
    },
    /// LLM generation completed (fires during progress callback, before AgentResponse)
    Completed {
        session_id: String,
        agent_id: String,
        response: String,
    },
    /// Agent encountered a processing error during message handling
    ProcessingError {
        session_id: String,
        agent_id: String,
        message: String,
    },
    /// Cron job announcement scheduled for delivery
    CronAnnounce {
        channel: String,
        to: String,
        message: String,
    },
    /// Agent is thinking/generating response (typing indicator)
    Thinking {
        session_id: String,
        agent_id: String,
        content: Option<String>,
    },
    /// Streaming text content delta (for real-time typing effect)
    ContentDelta {
        session_id: String,
        agent_id: String,
        delta: String,
    },
    /// Device status changed (connected, disconnected, error, degraded).
    DeviceStatusChanged {
        device_id: String,
        status: String,
        message: Option<String>,
    },
}

/// Agent status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStatus {
    Idle,
    Processing { session_id: String },
    Error(String),
    Shutdown,
}

/// Per-target restart tracking record (agent or channel)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairRecord {
    pub target: String,
    pub restart_count: u32,
    pub last_restart_at: Option<chrono::DateTime<chrono::Utc>>,
    pub abandoned: bool,
}

impl RepairRecord {
    pub fn new(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            restart_count: 0,
            last_restart_at: None,
            abandoned: false,
        }
    }
}

/// Shared state for the gateway-level self-repair loop — exposed via REST
pub struct RepairState {
    pub last_cycle_at: RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    pub records: RwLock<HashMap<String, RepairRecord>>,
    pub loop_running: std::sync::atomic::AtomicBool,
}

impl Default for RepairState {
    fn default() -> Self {
        Self::new()
    }
}

impl RepairState {
    pub fn new() -> Self {
        Self {
            last_cycle_at: RwLock::new(None),
            records: RwLock::new(HashMap::new()),
            loop_running: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

/// Query parameters for WebSocket connection
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    /// Start a new conversation (true/false)
    pub new: Option<bool>,
    /// Specific conversation ID to resume
    pub conversation: Option<String>,
}

/// Request body for switching default model
#[derive(Debug, Deserialize)]
pub struct SwitchModelRequest {
    /// Model alias to switch to (e.g., "fast", "smart", "default")
    pub model: String,
}

/// Request body for provider override in messages
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    /// Message content
    pub message: String,
    /// Optional caller user ID (falls back to "api_user")
    pub user_id: Option<String>,
    /// Optional provider override (e.g., "anthropic", "openai")
    pub provider_override: Option<String>,
    /// Optional model alias override (e.g., "fast", "smart")
    pub model_alias: Option<String>,
    /// Optional specific model ID override
    pub model_id: Option<String>,
}

/// Gateway control plane
pub struct Gateway {
    state: Arc<GatewayState>,
    config: GatewayConfig,
    shutdown_token: CancellationToken,
    /// Background tasks spawned by `Gateway::new()` and `Gateway::start()`.
    /// Drained and aborted during `stop()`.
    background_tasks: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
    /// Handles for the unified inbound/routed message workers.
    /// These are drained gracefully by closing their entry channels.
    message_workers: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
    /// Handles for each spawned agent processing loop.
    agent_tasks: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
    /// Physical device registry, populated when device drivers are provided.
    device_registry: Option<Arc<crate::device::registry::DeviceRegistry>>,
    /// Perception fusion layer registry, populated when perception is enabled.
    perception_registry: Option<Arc<crate::perception::PerceptionRegistry>>,
}

/// Initialize the perception fusion layer.
///
/// 1. Creates the [`PerceptionRegistry`] with the configured aggregation strategy.
/// 2. Registers computer adapter sources (screenshot, system monitor).
/// 3. Registers device capability sources (from the device subsystem).
/// 4. Registers the microphone source (if enabled).
/// 5. Registers the [`PerceptionQueryTool`] with [`FusionEngine`].
/// 6. Spawns a background poll loop (if configured).
/// 7. Stores the [`PerceptionInit`] on `state.perception_init`.
///
/// Returns `Some(Arc<PerceptionRegistry>)` when perception is enabled,
/// `None` otherwise.
async fn init_perception(
    config: &PerceptionConfig,
    state: &GatewayState,
    background_tasks: &mut Vec<JoinHandle<()>>,
    device_registry: Option<Arc<crate::device::registry::DeviceRegistry>>,
) -> Option<Arc<crate::perception::PerceptionRegistry>> {
    if !config.enabled {
        return None;
    }

    // Build the persistence backend (defaults to NullObservationStore).
    let store: Arc<dyn crate::perception::ObservationStore> = match config.persistence_backend.as_str() {
        "jsonl" => {
            let dir = config.persistence_dir.clone().map(std::path::PathBuf::from);
            match crate::perception::build_store("jsonl", dir).await {
                Ok(s) => {
                    tracing::info!(
                        "perception persistence: jsonl backend at {}",
                        config
                            .persistence_dir
                            .clone()
                            .unwrap_or_else(|| "<default temp dir>".into()),
                    );
                    s
                }
                Err(e) => {
                    tracing::warn!("failed to open jsonl perception store: {}; falling back to none", e);
                    Arc::new(crate::perception::NullObservationStore)
                }
            }
        }
        _ => Arc::new(crate::perception::NullObservationStore),
    };

    let reg = Arc::new(
        crate::perception::PerceptionRegistry::new(
            crate::perception::AggregationStrategy::Latest,
            config.aggregation_window_secs,
        )
        .with_store(store.clone()),
    );

    // Register computer adapter sources
    let computer_adapter = state.tools.computer_adapter.read().await.clone();
    if let Some(ref adapter) = computer_adapter {
        reg.register_source(Arc::new(
            crate::perception::ScreenshotAdapter::new(adapter.clone()),
        ))
        .await;

        let monitor = Arc::new(tokio::sync::Mutex::new(
            crate::computer::system::SystemMonitor::new(),
        ));
        reg.register_source(Arc::new(
            crate::perception::SystemMonitorAdapter::new(monitor),
        ))
        .await;
    }

    // Register device capabilities as perception sources
    if let Some(ref device_registry) = device_registry {
        for device_id in device_registry.list().await {
            if let Some(device) = device_registry.get(&device_id).await {
                for cap in &device.capabilities {
                    reg.register_source(Arc::new(
                        crate::perception::DeviceSourceAdapter::new(
                            device.id().to_string(),
                            cap.clone(),
                        ),
                    ))
                    .await;
                }
            }
        }
    }

    // Register microphone as a perception source
    if config.enable_microphone {
        let audio_source = match config.audio_source.as_str() {
            "system_output" => crate::computer::audio::AudioSource::SystemOutput,
            _ => crate::computer::audio::AudioSource::Microphone,
        };
        let adapter_config = crate::perception::AudioAdapterConfig {
            audio_source,
            sample_rate: config.audio_sample_rate,
            silence_threshold_db: config.silence_threshold_db,
            channel_capacity: 256,
            reprobe_interval_secs: 0,
        };
        reg.register_source(Arc::new(
            crate::perception::MicrophoneAdapter::new(adapter_config),
        ))
        .await;
        tracing::info!(
            "Microphone perception source registered (source={}, rate={}Hz)",
            config.audio_source,
            config.audio_sample_rate,
        );
    }

    // Register the perception query tool with fusion support
    let tool = Arc::new(
        crate::tools::perception_tool::PerceptionQueryTool::new(reg.clone())
            .with_fusion(crate::perception::FusionConfig::default()),
    );
    state.tools.registry.register_dynamic(tool);

    // Spawn background poll loop
    if config.poll_interval_secs > 0 {
        let r = reg.clone();
        let interval = std::time::Duration::from_secs(config.poll_interval_secs);
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                r.poll_all().await;
            }
        });
        background_tasks.push(handle);
    }

    // Spawn periodic prune task for the persistent store.
    if config.persistence_retention_days > 0
        && config.persistence_backend != "none"
    {
        let store_clone = store.clone();
        let retention_days = config.persistence_retention_days;
        let handle = tokio::spawn(async move {
            // Run every 6 hours.
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
            ticker.tick().await; // skip the immediate tick
            loop {
                ticker.tick().await;
                let cutoff = std::time::SystemTime::now()
                    - std::time::Duration::from_secs(retention_days * 86_400);
                match store_clone.prune_older_than(cutoff).await {
                    Ok(n) => tracing::debug!("perception store prune: ~{n} rows removed"),
                    Err(e) => tracing::warn!("perception store prune failed: {e}"),
                }
            }
        });
        background_tasks.push(handle);
    }

    // Spin up the streaming pipeline (raw_hub → temporal/fusion →
    // derived_hub) that per-agent MinimalAdapters subscribe to. Then
    // attach every streaming source already known to the registry, and
    // spawn a periodic sync so hot-plugged sources are picked up.
    let perception_context = Arc::new(crate::perception::PerceptionContext::start(
        crate::perception::PerceptionContextConfig::default(),
    ));
    perception_context
        .raw_hub()
        .sync_with_registry(reg.as_ref())
        .await;
    // Bridge poll-only sources (Screenshot, SystemMonitor, …) into
    // raw_hub by handing the registry the hub's broadcast sender.
    reg.set_raw_hub_sender(Some(perception_context.raw_hub().sender()))
        .await;
    // Wire health-transition anomalies onto the derived hub so per-agent
    // adapters see source faults (Healthy → Degraded → Quarantined and
    // back) without polling the health endpoint.
    reg.set_derived_hub_sender(Some(perception_context.derived_hub().sender()))
        .await;
    let sync_handle = crate::perception::spawn_stream_hub_sync(
        perception_context.raw_hub().clone(),
        reg.clone(),
        std::time::Duration::from_secs(5),
    );
    background_tasks.push(sync_handle);

    // Store PerceptionInit on state (poll_handle is tracked via background_tasks)
    *state.perception_init.write().await = Some(crate::gateway::state::PerceptionInit {
        registry: reg.clone(),
        context: perception_context,
        poll_handle: None,
    });

    Some(reg)
}

/// Initialize the control lane (optional high-priority safety loop).
///
/// When [`DeviceConfig::control`] is enabled, this function:
/// 1. Creates a [`ControlHandlerRegistry`].
/// 2. Collects [`ControlHandler`]s from registered device drivers.
/// 3. Spawns the control loop on the current runtime (or a dedicated
///    single-threaded runtime if configured).
/// 4. Stores a [`ControlInit`] on `state.control_init`.
async fn init_control(
    device_config: &crate::gateway::DeviceConfig,
    state: &GatewayState,
) {
    if !device_config.control.enabled {
        return;
    }

    // Get the device registry — need it for the control loop.
    let registry = match state.device_init.read().await.as_ref() {
        Some(di) => di.registry.clone(),
        None => {
            tracing::warn!("Control lane enabled but no device registry available");
            return;
        }
    };

    let handlers = crate::device::control::new_handler_registry();

    // Collect control handlers from registered device drivers.
    // Only drivers that implement `control_handler()` on DeviceDriver
    // will contribute handlers.
    // (Handlers are registered by driver name, then mapped to device IDs
    //  at runtime by the control loop.)
    // For now, the control loop only uses the registry for health checks;
    // handler registration will be expanded in future iterations.

    // Spawn the control loop on the current runtime.
    let handle = crate::device::control::spawn_control_loop(
        registry.clone(),
        handlers.clone(),
        device_config.control.clone(),
    );

    *state.control_init.write().await = Some(crate::gateway::state::ControlInit {
        registry,
        runtime: None,
        handle: Some(handle),
        handlers,
    });

    tracing::info!(
        "Control lane initialized (interval: {}ms)",
        device_config.control.loop_interval_ms,
    );
}

/// Validate authentication configuration for ambiguity and conflicts.
///
/// Fails fast when both `shared_token` and OAuth providers are configured
/// but `auth_mode` is not explicitly set.
fn validate_auth_config(config: &GatewayConfig) -> crate::Result<()> {
    if !config.security.enabled || !config.security.auth_required {
        return Ok(());
    }

    let has_token = config.security.shared_token.is_some();
    let has_oauth = config.security.oauth.enabled
        && (config.security.oauth.github.is_some() || config.security.oauth.google.is_some());
    let mode_unset = config.security.auth_mode == crate::gateway::protocol::AuthMode::None;

    if has_token && has_oauth && mode_unset {
        return Err(crate::error::SyscityError::Validation(
            "Auth mode ambiguity: both shared_token and OAuth are configured but \
             auth_mode is not set. Please set auth_mode to 'token' or 'device' \
             in your security configuration."
                .into(),
        ));
    }

    // Warn when token is configured but auth_mode is not Token
    if has_token && mode_unset && config.security.shared_token.as_deref() != Some("") {
        tracing::warn!(
            "shared_token is configured but auth_mode is 'none'. \
             Set auth_mode to 'token' for consistent authentication."
        );
    }

    Ok(())
}

impl Gateway {
    /// Create a new gateway instance with no device drivers.
    pub async fn new(config: GatewayConfig, config_path: Option<PathBuf>) -> crate::Result<Self> {
        Self::with_devices(config, config_path, vec![]).await
    }

    /// Create a new gateway instance with optional device drivers.
    ///
    /// Device drivers are discovered, probed, and connected at startup.  Each
    /// capability is registered in `ToolRegistry` so the LLM can discover and call device operations through standard
    /// function calling.
    ///
    /// Pass an empty vec (or use [`Gateway::new`]) when no physical devices
    /// are needed.
    pub async fn with_devices(
        config: GatewayConfig,
        config_path: Option<PathBuf>,
        device_drivers: Vec<Arc<dyn crate::device::DeviceDriver>>,
    ) -> crate::Result<Self> {
        // Validate security configuration before proceeding
        validate_auth_config(&config)?;

        let (event_tx, _) = broadcast::channel(1000);
        let (log_tx, _) = broadcast::channel(1000);
        let (inbound_entry_tx, inbound_entry_rx) =
            mpsc::channel::<crate::channels::IncomingMessage>(1000);
        let (routed_tx, routed_rx) = mpsc::channel(1000);
        let shutdown_token = CancellationToken::new();

        // Initialize storage adapter, shared SQLite pool, session store, and audit log
        let storage_init = init::storage::init_storage(&config).await?;
        let storage = storage_init.storage;
        let unified_vector_store = storage_init.unified_vector_store;
        let sqlite_pool = storage_init.sqlite_pool;
        let session_store = storage_init.session_store;
        let audit_log = storage_init.audit_log;
        let audit_log_dyn = storage_init.audit_log_dyn;

        // Initialize ACP control plane and model router
        let acp = init::agents::init_acp(&config, session_store.clone()).await;
        let model_router = init::agents::init_model_router(&config).await;

        // Initialize skill manager, agent registry, and session manager
        let (skills_manager, agent_registry, session_manager) =
            init::agents::init_agent_state().await?;

        // Initialize tool subsystem (registry, MCP, plugins, channels, computer adapter)
        let tools_init = init::tools::init_tools(
            &config,
            acp.clone(),
            session_store.clone(),
            audit_log_dyn.clone(),
            model_router.clone(),
        )
        .await?;

        // Configure ACP default agent builder now that provider and tools are ready
        init::agents::configure_acp_agent_builder(
            &acp,
            &config,
            model_router.clone(),
            tools_init.tool_registry.clone(),
            skills_manager.clone(),
        )
        .await;

        // Initialize security components
        let security_init = init::security::init_security(&config, audit_log_dyn.clone()).await?;

        // Initialize inbound / outbound pipelines
        let pipelines_init = init::pipelines::init_pipelines(
            &config,
            sqlite_pool.as_ref(),
            model_router.clone(),
            routed_tx.clone(),
        )
        .await?;

        // Assemble the domain-grouped GatewayState used by the rest of the system
        let state = Arc::new(GatewayState {
            config: Arc::new(RwLock::new(config.clone())),
            start_time: Instant::now(),
            config_path: config_path.clone(),
            device_init: RwLock::new(None),
            perception_init: RwLock::new(None),
            control_init: RwLock::new(None),
            auth: AuthState {
                manager: security_init.auth_manager.clone(),
                pairing_store: Arc::new(crate::security::pairing::PairingStore::new()),
                device_pairing_store: Arc::new(
                    crate::security::device_pairing::DevicePairingStore::new(),
                ),
                tailscale_authenticator: {
                    let ttl = config.security.tailscale_auth_ttl_secs;
                    Some(Arc::new(crate::security::tailscale::TailscaleAuthenticator::new(
                        std::time::Duration::from_secs(ttl),
                    )))
                },
                trusted_proxy_authenticator: {
                    let tp_config = config.security.trusted_proxy.clone();
                    if tp_config.enabled {
                        Some(Arc::new(
                            crate::security::trusted_proxy::TrustedProxyAuthenticator::new(
                                tp_config,
                            ),
                        ))
                    } else {
                        None
                    }
                },
                rate_limiter: security_init.rate_limiter.clone(),
                multi_tier_rate_limiter: security_init.multi_tier_rate_limiter.clone(),
                audit_log: audit_log.clone(),
                command_gate: security_init.command_gate.clone(),
                mention_gate: security_init.mention_gate.clone(),
            },
            agents: AgentState {
                agents: Arc::new(RwLock::new(HashMap::new())),
                router: pipelines_init.agent_router.clone(),
                registry: agent_registry.clone(),
                manager: session_manager.clone(),
                group_manager: Arc::new(RwLock::new(crate::agent::GroupSessionManager::new())),
                store: session_store.clone(),
                message_buffer: Arc::new(RwLock::new(HashMap::new())),
                route_resolver: Arc::new(crate::agent::RouteResolver::new("default")),
                cost_guard: crate::agent::CostGuard::new(
                    config.cost_guard.daily_limit_cents,
                    config.cost_guard.hourly_action_limit,
                ),
                repair_state: Arc::new(RepairState::new()),
                acp: acp.clone(),
                session_routing: Arc::new(RwLock::new(HashMap::new())),
            },
            channels: ChannelState {
                channels: tools_init.channels.clone(),
                extensions: tools_init.channel_extensions.clone(),
                reply_dispatcher: pipelines_init.reply_dispatcher.clone(),
                snapshot_store: None,
                health_monitor: None,
                acp_bridge: None,
                session_channels: Arc::new(RwLock::new(HashMap::new())),
                webhook_sessions: Arc::new(RwLock::new(HashMap::new())),
            },
            memory: MemoryState {
                vector: crate::utils::LateInit::new(),
                session_search: crate::utils::LateInit::new(),
                manager: tools_init.memory_manager_holder.clone(),
                dream_scheduler: crate::utils::LateInit::new(),
                dream_metrics: Arc::new(crate::memory::DreamMetrics::default()),
                standing_order_manager: crate::utils::LateInit::new(),
            },
            tools: ToolState {
                registry: tools_init.tool_registry.clone(),
                mcp_manager: tools_init.mcp_manager.clone(),
                approval_queue: tools_init.approval_queue.clone(),
                skills_manager: skills_manager.clone(),
                canvas_manager: tools_init.canvas_manager.clone(),
                computer_adapter: Arc::new(tokio::sync::RwLock::new(tools_init.computer_adapter.clone())),
            },
            pipelines: PipelineState {
                inbound: pipelines_init.inbound_pipeline.clone(),
                outbound: pipelines_init.outbound_pipeline.clone(),
                side_effect_executor: pipelines_init.side_effect_executor.clone(),
                sse_streamer: pipelines_init.sse_streamer.clone(),
                routed_tx: routed_tx.clone(),
                inbound_entry: inbound_entry_tx.clone(),
            },
            events: EventState {
                tx: event_tx.clone(),
                log_tx: log_tx.clone(),
                hook_registry: Arc::new(hooks::EventHookRegistry::new()),
            },
            infra: InfraState {
                storage: storage.clone(),
                runtime_settings: Arc::new(RwLock::new(HashMap::new())),
                transcript_store: {
                    let store = crate::agent::TranscriptStore::new(crate::dirs::transcripts_dir());
                    Arc::new(store)
                },
                artifact_store: {
                    let store = crate::agent::ArtifactStore::new(crate::dirs::artifacts_dir());
                    Arc::new(store)
                },
                disk_budget: {
                    let manager = crate::agent::DiskBudgetManager::new(crate::dirs::budget_dir());
                    let _ = manager.init();
                    Arc::new(manager)
                },
                session_file_manager: {
                    let manager =
                        crate::agent::SessionFileManager::new(crate::dirs::session_files_dir());
                    let _ = manager.init().await;
                    Arc::new(manager)
                },
                hot_reload: crate::utils::LateInit::new(),
                plugin_manager: tools_init.plugin_manager.clone(),
                driver_factory: crate::device::DriverFactory::new(),
                model_router: model_router.clone(),
                engine_metrics: None,
                #[cfg(feature = "browser")]
                browser_bridge: tokio::sync::RwLock::new(None),
            },
            sdk: SdkState {
                provider_sdk: Arc::new(RwLock::new(crate::providers::ProviderSdk::new())),
                tool_sdk: Arc::new(RwLock::new(crate::tools::ToolSdk::new())),
            },
            scheduler: SchedulerState {
                task_scheduler: crate::utils::LateInit::new(),
                heartbeat_wake_tx: crate::utils::LateInit::new(),
                heartbeat_event_tx: crate::utils::LateInit::new(),
                cron_scheduler: crate::utils::LateInit::new(),
            },
        });

        // Background tasks spawned before `Gateway` is fully constructed are
        // collected here and then handed off to the `background_tasks` field.
        let mut background_tasks: Vec<JoinHandle<()>> = Vec::new();

        // Attach SessionStore to SessionManager for unified session model
        if let Some(ref store) = state.agents.store {
            let mut mgr = state.agents.manager.write().await;
            mgr.with_store(store.clone());
        }

        // Wire ACP lifecycle events into the gateway broadcast channel
        state.agents.acp.set_event_tx(state.events.tx.clone()).await;

        // Forward MCP lifecycle events into the gateway broadcast channel
        {
            let event_tx = state.events.tx.clone();
            let mut mcp_event_rx = tools_init.mcp_event_rx;
            let mcp_forward_handle = tokio::spawn(async move {
                while let Some(event) = mcp_event_rx.recv().await {
                    let gateway_event = match event {
                        crate::tools::mcp::McpEvent::Connected {
                            server_id,
                            tools,
                            prompts,
                            resources,
                        } => GatewayEvent::McpConnected {
                            server_id,
                            tools,
                            prompts,
                            resources,
                        },
                        crate::tools::mcp::McpEvent::Disconnected { server_id, reason } => {
                            GatewayEvent::McpDisconnected { server_id, reason }
                        }
                        crate::tools::mcp::McpEvent::Recovered { server_id, attempt } => {
                            GatewayEvent::McpRecovered { server_id, attempt }
                        }
                        crate::tools::mcp::McpEvent::ResourceChanged { server_id, uri } => {
                            GatewayEvent::McpResourceChanged { server_id, uri }
                        }
                    };
                    let _ = event_tx.send(gateway_event);
                }
            });
            background_tasks.push(mcp_forward_handle);
        }

        // Initialize audit table (SQLite-backed persistent audit log)
        if let Err(e) = state.auth.audit_log.init().await {
            warn!("Failed to initialize persistent audit log: {}", e);
        }

        // Dynamically register tools that need GatewayState
        state.tools.registry
            .register_dynamic(Arc::new(crate::tools::AgentsListTool::new(
                state.agents.registry.clone(),
            )));
        state.tools.registry
            .register_dynamic(Arc::new(crate::tools::GatewayTool::new(state.clone())));
        state.tools.registry
            .register_dynamic(Arc::new(crate::tools::MessageTool::new(state.clone())));
        state.tools.registry
            .register_dynamic(Arc::new(crate::tools::CanvasTool::new(
                state.tools.canvas_manager.clone(),
            )));

        // Sync ProviderSdk / ToolSdk with existing registries
        {
            let mut provider_sdk = state.sdk.provider_sdk.write().await;
            provider_sdk
                .sync_from_model_router(&state.infra.model_router)
                .await;
        }
        {
            let mut tool_sdk = state.sdk.tool_sdk.write().await;
            tool_sdk.sync_from_tool_registry(&state.tools.registry);
        }

        // Initialize late services: vector memory, session search, cron, task scheduler, etc.
        init::services::init_late_services(
            &config,
            &state,
            sqlite_pool.as_ref(),
            unified_vector_store,
        )
        .await?;

        // ── Initialize device subsystem ──
        // Probes, connects, and registers device capabilities as tools in
        // ToolRegistry so the LLM can discover and call device operations
        // through standard function calling.
        //
        // Drivers are provided either explicitly (via `with_devices()`) or
        // discovered from the configuration's `device.drivers` entries.
        //
        // The shared DriverFactory is stored in state so that all paths
        // (config-driven init, OS bridge, hot-reload, native plugins) use
        // the same registered constructors (already initialized in InfraState).

        let device_drivers = if device_drivers.is_empty() {
            crate::gateway::init::devices::discover_drivers_from_config(
                &state.infra.driver_factory,
                &config.device,
            )
        } else {
            device_drivers
        };

        // Scan native plugin directory if configured
        #[cfg(feature = "native-plugins")]
        if let Some(ref dir) = config.device.native_plugins_dir {
            tracing::info!("Scanning native plugins directory: {:?}", dir);
            state.infra.driver_factory.scan_native_plugins_dir(dir);
        }

        let mut device_init = crate::gateway::init::devices::init_devices(
            &config.device,
            device_drivers,
            &state.tools.registry,
            None,
        )
        .await?;

        // Spawn OS device bridge if enabled
        if let Some(ref mut di) = device_init {
            if let Some(handle) = crate::gateway::init::devices::spawn_os_bridge_from_config(
                &state.infra.driver_factory,
                di.registry.clone(),
                &config.device.os_bridge,
                state.tools.registry.clone(),
                None,
            ) {
                di.os_bridge_handle = Some(handle);
            }
        }

        // Store device_init on state for lifecycle management and hot-reload
        // The health check handle lives on DeviceInit, not background_tasks,
        // so it can be aborted during hot-reload without ownership conflicts.
        *state.device_init.write().await = device_init;

        let device_registry: Option<Arc<crate::device::registry::DeviceRegistry>> =
            state.device_init.read().await.as_ref().map(|di| di.registry.clone());

        // Initialize perception fusion layer (delegated to helper)
        let perception_registry: Option<Arc<crate::perception::PerceptionRegistry>> =
            init_perception(
                &config.perception,
                state.as_ref(),
                &mut background_tasks,
                device_registry.clone(),
            )
            .await;

        // Initialize control lane (optional, runs alongside perception)
        init_control(&config.device, state.as_ref()).await;

        // Start message processing workers
        let inbound_handle = tokio::spawn(Self::process_inbound_entries(
            state.clone(),
            inbound_entry_rx,
            shutdown_token.clone(),
        ));
        let routed_handle = tokio::spawn(Self::process_routed_messages(
            state.clone(),
            routed_rx,
            shutdown_token.clone(),
        ));

        Ok(Self {
            state,
            config,
            shutdown_token,
            background_tasks: tokio::sync::Mutex::new(background_tasks),
            message_workers: tokio::sync::Mutex::new(vec![inbound_handle, routed_handle]),
            agent_tasks: tokio::sync::Mutex::new(Vec::new()),
            device_registry,
            perception_registry,
        })
    }

    /// Return a clone of the internal `ModelRouter` arc.
    ///
    /// Primarily used in integration / E2E tests to inject a mock provider
    /// before calling `start()`.
    pub fn model_router(&self) -> Arc<crate::model_router::ModelRouter> {
        self.state.infra.model_router.clone()
    }

    /// Return a clone of the internal `ToolRegistry` arc.
    pub fn tool_registry(&self) -> Arc<crate::tools::ToolRegistry> {
        self.state.tools.registry.clone()
    }

    /// Return a clone of the internal `DeviceRegistry`, if one exists.
    ///
    /// Returns `None` when no device drivers were registered at startup.
    /// Primarily used in integration / E2E tests to verify device registration.
    pub fn device_registry(&self) -> Option<Arc<crate::device::registry::DeviceRegistry>> {
        self.device_registry.clone()
    }

    /// Return a clone of the internal `PerceptionRegistry`, if one exists.
    /// Returns `None` when perception is disabled.
    pub fn perception_registry(&self) -> Option<Arc<crate::perception::PerceptionRegistry>> {
        self.perception_registry.clone()
    }

    /// Return a clone of the gateway shutdown token.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown_token.clone()
    }

    /// Spawn a background task and track it for graceful shutdown.
    async fn spawn_task(&self, handle: JoinHandle<()>) {
        self.background_tasks.lock().await.push(handle);
    }

    /// Spawn an agent processing loop and track it for graceful shutdown.
    async fn spawn_agent_task(&self, handle: JoinHandle<()>) {
        self.agent_tasks.lock().await.push(handle);
    }

    /// Start the gateway
    pub async fn start(&self) -> crate::Result<()> {
        info!("Starting Syscity Gateway control plane...");

        // Initialize plugins if enabled
        if self.config.plugins.enabled {
            if self.config.plugins.auto_load {
                if let Err(e) = self.state.infra.plugin_manager.initialize().await {
                    warn!("Failed to initialize plugins: {}", e);
                }

                // Watch WASM files for hot-reload
                if let Some(hot_reload) = self.state.infra.hot_reload.get_opt().await {
                    let plugins = self.state.infra.plugin_manager.list_plugins().await;
                    for plugin in plugins {
                        if let Some(ref main) = plugin.manifest.main {
                            let wasm_path = plugin.path.join(main);
                            if wasm_path.exists() {
                                if let Err(e) = hot_reload
                                    .watch_file(&wasm_path, ConfigFileType::Plugin)
                                    .await
                                {
                                    warn!(
                                        "Failed to watch WASM file for plugin '{}': {}",
                                        plugin.id(),
                                        e
                                    );
                                } else {
                                    debug!(
                                        "Watching WASM file for plugin '{}': {:?}",
                                        plugin.id(),
                                        wasm_path
                                    );
                                }
                            }
                        }
                    }
                }
            } else {
                info!("Plugin auto-load disabled, skipping initialization");
            }
        } else {
            info!("Plugin system disabled");
        }

        // Initialize skills manager
        {
            let mut skills_manager = self.state.tools.skills_manager.write().await;
            match skills_manager.initialize().await {
                Ok(count) => info!("✅ Skills manager initialized with {} skills", count),
                Err(e) => warn!("Failed to initialize skills manager: {}", e),
            }
        }

        // Initialize hot reload if enabled
        let hot_reload = self.state.infra.hot_reload.get_opt().await;
        if let Some(ref hot_reload) = hot_reload {
            let config_path = crate::dirs::default_config_file();
            if let Err(e) = hot_reload
                .watch_file(&config_path, ConfigFileType::Main)
                .await
            {
                warn!("Failed to watch config file: {}", e);
            }
            // Start hot reload processing in background
            let hot_reload_clone = hot_reload.clone();
            let hot_reload_handle = tokio::spawn(async move {
                if let Err(e) = hot_reload_clone.run().await {
                    error!("Hot reload error: {}", e);
                }
            });
            self.spawn_task(hot_reload_handle).await;

            // Register config change handlers
            self.register_hot_reload_handlers(hot_reload).await;
        }

        // Initialize default agent (optional - requires provider configuration)
        let mut default_config = self.config.default_agent.clone();
        let default_agent_dir = crate::dirs::agents_dir().join("default");
        default_config.system_prompt = format!(
            "{}\n\n## Agent Identity\n\nYour agent ID is: `default`\nYour agent directory is: `{}`\nYou may edit files in your agent directory (including HEARTBEAT.md) to manage your personality and periodic tasks when explicitly asked by the user.",
            default_config.system_prompt,
            default_agent_dir.display()
        );
        match self
            .spawn_agent("default".to_string(), default_config)
            .await
        {
            Ok(()) => info!("Default agent spawned successfully"),
            Err(e) => {
                warn!("Failed to spawn default agent: {}", e);
                warn!("Gateway running without default agent - agents must be created via API");
            }
        }

        // Discover agents from agents/ directory (auto-discovery)
        {
            let mut registry = self.state.agents.registry.write().await;
            match registry.discover().await {
                Ok(count) => {
                    if count > 0 {
                        info!("🔍 Discovered {} agents from agents/ directory", count);
                        // List discovered agents
                        for id in registry.list() {
                            if let Some(personality) = registry.get(&id) {
                                info!("  📋 Agent '{}' - {}", id, personality.display_name());
                            }
                        }
                    } else {
                        info!("🔍 No agents found in agents/ directory");
                    }
                }
                Err(e) => {
                    warn!("Failed to discover agents: {}", e);
                }
            }
        }

        // Register delegation tool with agent resolver for target_agent routing.
        // The resolver consults the running agents map so that `target_agent`
        // in TaskSpec routes children to the appropriate specialised agent.
        {
            use crate::tools::DelegateTool;
            let resolver = Arc::new(GatewayAgentResolver {
                agents: self.state.agents.agents.clone(),
            });
            let default_agent = {
                let agents = self.state.agents.agents.read().await;
                agents.get("default").map(|h| h.agent.clone())
            };
            let delegate = if let Some(agent) = default_agent {
                DelegateTool::with_agent(0, agent).with_agent_resolver(resolver)
            } else {
                DelegateTool::root().with_agent_resolver(resolver)
            };
            self.state.tools.registry
                .register_dynamic(Arc::new(delegate));
            info!("DelegateTool registered with agent resolver for target_agent routing");
        }

        // Auto-connect MCP servers (9.1, 9.2)
        self.init_mcp_servers().await;

        // Initialize configured channels
        self.init_channels().await?;

        // Start dream scheduler if enabled
        if self.config.dreaming.enabled {
            if let Some(mm) = self.state.memory.manager.read().await.as_ref().cloned() {
                if let Some(tier_index) = mm.tier_index() {
                    let dreaming = &self.config.dreaming;
                    // Convert string-based MemoryDreamingConfig to enum-based DreamConfig
                    let speed = match dreaming.speed.to_lowercase().as_str() {
                        "fast" => crate::memory::DreamSpeed::Fast,
                        "slow" => crate::memory::DreamSpeed::Slow,
                        _ => crate::memory::DreamSpeed::Balanced,
                    };
                    let thinking = match dreaming.thinking.to_lowercase().as_str() {
                        "low" => crate::memory::DreamThinking::Low,
                        "high" => crate::memory::DreamThinking::High,
                        _ => crate::memory::DreamThinking::Medium,
                    };
                    let budget = match dreaming.budget.to_lowercase().as_str() {
                        "cheap" => crate::memory::DreamBudget::Cheap,
                        "expensive" => crate::memory::DreamBudget::Expensive,
                        _ => crate::memory::DreamBudget::Medium,
                    };
                    let dream_config = crate::memory::DreamConfig {
                        enabled: dreaming.enabled,
                        frequency: dreaming.frequency.clone(),
                        speed,
                        thinking,
                        budget,
                        dedup_similarity_threshold: dreaming.dedup_similarity_threshold,
                        ..crate::memory::DreamConfig::default()
                    };
                    let tier_system_config = crate::memory::TierSystemConfig::default();
                    let mut engine =
                        crate::memory::DreamEngine::new(dream_config, tier_system_config)
                            .with_metrics(Arc::clone(&self.state.memory.dream_metrics));
                    if let Some(ref workspace_dir) = self.config.workspace_dir {
                        engine = engine.with_workspace_dir(workspace_dir.clone());
                    }
                    if let Some(event_log) = mm.event_log() {
                        engine = engine.with_event_log(event_log.clone());
                    }
                    engine.initialize().await;
                    let engine = Arc::new(engine);
                    let mut scheduler = crate::memory::DreamScheduler::new(engine);
                    scheduler.start(mm.store(), tier_index);
                    info!("Dream scheduler started");
                    self.state.memory.dream_scheduler.init(scheduler).await;
                }
            }
        }

        // Start standing orders manager if configured
        if self.config.standing_orders.enabled {
            let mut manager = crate::standing_orders::StandingOrderManager::new(
                self.config.standing_orders.clone(),
                self.state.clone(),
            );
            manager.start();
            info!("Standing orders manager started");
            self.state.memory.standing_order_manager.init(manager).await;
        }

        // Start browser bridge server if enabled
        #[cfg(feature = "browser")]
        if self.config.browser.bridge_enabled {
            let pool = Arc::new(crate::browser::BrowserPool::with_profiles(
                self.config.browser.pool.clone(),
                self.config.browser.profiles.clone(),
            ));
            let mut bridge =
                crate::browser::BrowserBridge::new(pool, self.config.browser.bridge_port);
            let token = bridge.token().to_string();
            match bridge.start().await {
                Ok(port) => {
                    let url = format!("http://127.0.0.1:{}", port);
                    info!(port = port, "Browser bridge server started");
                    {
                        let mut bridge_lock = self.state.infra.browser_bridge.write().await;
                        *bridge_lock = Some(bridge);
                    }
                    let mut settings = self.state.infra.runtime_settings.write().await;
                    settings.insert("browser_bridge_url".to_string(), serde_json::json!(url));
                    settings.insert("browser_bridge_token".to_string(), serde_json::json!(token));
                }
                Err(e) => {
                    warn!("Failed to start browser bridge server: {}", e);
                }
            }
        }

        // Build HTTP router
        let app = self.build_router().await;

        // Bind to address
        let addr: SocketAddr = format!("{}:{}", self.config.host, self.config.port)
            .parse()
            .map_err(|e| crate::error::ConfigError::InvalidValue {
                key: "gateway.address".to_string(),
                message: format!("Invalid gateway address: {}", e),
            })?;

        let listener = TcpListener::bind(&addr).await.map_err(|e| {
            crate::error::SyscityError::ExternalService {
                source: "Failed to bind gateway".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

        info!("Gateway control plane listening on ws://{}", addr);

        // Start Tailscale if enabled
        #[cfg(feature = "tailscale")]
        if self.config.tailscale_enabled {
            self.start_tailscale().await?;
        }

        // Forward ApprovalRequired events from the tool registry into the Gateway event bus
        {
            let mut approval_rx = self.state.tools.approval_queue.event_tx.subscribe();
            let event_tx = self.state.events.tx.clone();
            let approval_handle = tokio::spawn(async move {
                while let Ok(evt) = approval_rx.recv().await {
                    let _ = event_tx.send(GatewayEvent::ApprovalRequired {
                        approval_id: evt.approval_id,
                        tool_name: evt.tool_name,
                        requested_by: evt.requested_by,
                        risk_level: evt.risk_level,
                        message: evt.message,
                    });
                }
            });
            self.spawn_task(approval_handle).await;
        }

        // Start gateway-level self-repair watchdog (60 s interval)
        let repair_handle = tokio::spawn(run_repair_loop(self.state.clone()));
        self.spawn_task(repair_handle).await;

        // Start heartbeat runner if enabled
        if self.config.heartbeat.enabled {
            let runner = crate::heartbeat::HeartbeatRunner::new(self.state.clone());
            let wake_tx = runner.wake_sender();
            let event_tx = runner.event_tx.clone();
            self.state.scheduler.heartbeat_wake_tx.init(wake_tx.clone()).await;
            self.state.scheduler.heartbeat_event_tx.init(event_tx).await;
            let heartbeat_handle = tokio::spawn(async move {
                runner.start().await;
            });
            self.spawn_task(heartbeat_handle).await;
            info!("Heartbeat runner started");

            // Wire heartbeat wake sender into cron scheduler so cron jobs
            // with wake_mode: heartbeat_nuke can trigger immediate heartbeats
            if let Some(cron_arc) = self.state.scheduler.cron_scheduler.get_opt().await {
                let mut scheduler = cron_arc.lock().await;
                scheduler.set_heartbeat_wake_tx(wake_tx);
                info!("Cron heartbeat wake integration enabled");
            }
        }

        // Start log tail broadcaster for real-time log streaming
        {
            let log_tx = self.state.events.log_tx.clone();
            let log_tail_handle = tokio::spawn(async move {
                let log_path = crate::logs::log_file_path();
                let mut pos: u64 = 0;
                loop {
                    if log_path.exists() {
                        match tokio::fs::metadata(&log_path).await {
                            Ok(meta) => {
                                let new_len = meta.len();
                                if new_len > pos {
                                    match tokio::fs::File::open(&log_path).await {
                                        Ok(file) => {
                                            let mut reader = tokio::io::BufReader::new(file);
                                            if let Err(e) =
                                                reader.seek(tokio::io::SeekFrom::Start(pos)).await
                                            {
                                                tracing::warn!("Log tail seek error: {}", e);
                                            } else {
                                                let mut lines = reader.lines();
                                                while let Ok(Some(line)) = lines.next_line().await {
                                                    let _ = log_tx.send(line);
                                                }
                                            }
                                            pos = new_len;
                                        }
                                        Err(e) => {
                                            tracing::warn!("Log tail open error: {}", e);
                                        }
                                    }
                                } else if new_len < pos {
                                    // File was truncated/rotated
                                    pos = 0;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Log tail metadata error: {}", e);
                            }
                        }
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            });
            self.spawn_task(log_tail_handle).await;
            info!("Log tail broadcaster started");
        }

        // Run the server with graceful shutdown so `Gateway::stop()` can end it.
        let shutdown_token = self.shutdown_token.clone();
        let shutdown = async move { shutdown_token.cancelled().await };
        axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Gateway server error".to_string(),
                cause: Some(Box::new(e)),
            })?;

        // Stop dream scheduler on shutdown
        if let Some(mut scheduler) = self.state.memory.dream_scheduler.get_opt().await {
            scheduler.stop().await;
            info!("Dream scheduler stopped");
        }

        // Stop standing orders manager on shutdown
        if let Some(mut manager) = self.state.memory.standing_order_manager.get_opt().await {
            manager.stop().await;
            info!("Standing orders manager stopped");
        }

        Ok(())
    }

    /// Gracefully shut down the gateway and its subsystems.
    ///
    /// The shutdown sequence follows the dependency order from the architecture
    /// plan: stop accepting new traffic, drain in-flight messages, stop agents,
    /// channels, ACP, cron, dream/standing-order schedulers, MCP, hot reload,
    /// task scheduler, browser/heartbeat/log-tail tasks, plugins, and finally
    /// background tasks.
    pub async fn stop(&self) -> crate::Result<()> {
        info!("Shutting down Syscity Gateway...");

        // Signal every cancel-aware loop to exit.
        self.shutdown_token.cancel();

        // 1. Drain the unified message workers.
        let message_handles = {
            let mut workers = self.message_workers.lock().await;
            std::mem::take(&mut *workers)
        };
        for handle in message_handles {
            match timeout(Duration::from_secs(5), handle).await {
                Ok(_) => {}
                Err(_) => warn!("Message worker did not stop within timeout"),
            }
        }

        // 2. Stop all spawned agents and await their loops.
        {
            let agents = self.state.agents.agents.read().await;
            for (_id, handle) in agents.iter() {
                let _ = handle.tx.send(crate::gateway::AgentCommand::Shutdown).await;
            }
        }
        let agent_handles = {
            let mut tasks = self.agent_tasks.lock().await;
            std::mem::take(&mut *tasks)
        };
        for handle in agent_handles {
            match timeout(Duration::from_secs(10), handle).await {
                Ok(_) => {}
                Err(_) => warn!("Agent task did not stop within timeout"),
            }
        }

        // 3. Stop configured channels.
        let channel_refs: Vec<Arc<dyn crate::channels::Channel>> = {
            let channels = self.state.channels.channels.read().await;
            channels.values().cloned().collect()
        };
        for channel in channel_refs {
            let name = channel.name().to_string();
            if let Err(e) = channel.stop().await {
                warn!("Failed to stop channel '{}': {}", name, e);
            } else {
                info!("Channel '{}' stopped", name);
            }
        }

        // 4. ACP shutdown.
        self.state.agents.acp.shutdown().await;
        info!("ACP control plane shut down");

        // 5. Cron scheduler.
        if let Some(cron_arc) = self.state.scheduler.cron_scheduler.get_opt().await {
            let mut scheduler = cron_arc.lock().await;
            if let Err(e) = scheduler.shutdown().await {
                warn!("Failed to shutdown cron scheduler: {}", e);
            } else {
                info!("Cron scheduler stopped");
            }
        }

        // 6. Dream scheduler.
        if let Some(mut scheduler) = self.state.memory.dream_scheduler.get_opt().await {
            scheduler.stop().await;
            info!("Dream scheduler stopped");
        }

        // 7. Standing orders manager.
        if let Some(mut manager) = self.state.memory.standing_order_manager.get_opt().await {
            manager.stop().await;
            info!("Standing orders manager stopped");
        }

        // 8. Disconnect MCP servers.
        let mcp_servers = self.state.tools.mcp_manager.list_servers().await;
        for server_id in mcp_servers {
            if let Err(e) = self.state.tools.mcp_manager.disconnect(&server_id).await {
                warn!("Failed to disconnect MCP server '{}': {}", server_id, e);
            }
        }

        // 9. Hot reload.
        if let Some(hot_reload) = self.state.infra.hot_reload.get_opt().await {
            if let Err(e) = hot_reload.stop().await {
                warn!("Failed to stop hot reload manager: {}", e);
            }
        }

        // 10. Task scheduler.
        if let Some(ts_arc) = self.state.scheduler.task_scheduler.get_opt().await {
            let mut scheduler = ts_arc.lock().await;
            if let Err(e) = scheduler.stop().await {
                warn!("Failed to stop task scheduler: {}", e);
            }
        }

        // 11. Browser bridge / pool.
        #[cfg(feature = "browser")]
        {
            let mut bridge_lock = self.state.infra.browser_bridge.write().await;
            if let Some(bridge) = bridge_lock.take() {
                bridge.shutdown().await;
                info!("Browser pool shut down");
            }
        }

        // 12. Tailscale.
        #[cfg(feature = "tailscale")]
        if self.config.tailscale_enabled {
            if let Err(e) = crate::tailscale::stop().await {
                warn!("Failed to stop Tailscale: {}", e);
            }
        }

        // 13. Abort remaining background tasks (log tail, heartbeat, repair loop,
        //     channel bridges, cron announce forwarder, etc.).
        let background_handles = {
            let mut tasks = self.background_tasks.lock().await;
            std::mem::take(&mut *tasks)
        };
        for handle in background_handles {
            handle.abort();
        }

        // 14. Abort device background handles (stored on state for hot-reload).
        {
            let mut di = self.state.device_init.write().await;
            if let Some(ref mut init) = *di {
                if let Some(handle) = init.health_check_handle.take() {
                    handle.abort();
                }
                if let Some(handle) = init.hot_plug_handle.take() {
                    handle.abort();
                }
                if let Some(handle) = init.os_bridge_handle.take() {
                    handle.abort();
                }
            }
        }

        // 15. Plugin manager shutdown.
        if let Err(e) = self.state.infra.plugin_manager.shutdown().await {
            warn!("Failed to shutdown plugin manager: {}", e);
        }

        // 16. Storage is left to flush on process exit because `dyn Storage`
        //     does not expose a close method.

        info!("Gateway shutdown complete");
        Ok(())
    }

    /// Build the HTTP router
    async fn build_router(&self) -> Router {
        let state = self.state.clone();

        // Public tier: Webhooks (no authentication, signature verification per-channel)
        let public_router = webhooks::create_webhook_router(state.clone());

        // Auth tier: OAuth login/logout (public-facing, no tailscale restriction)
        let auth_router = Router::new()
            .route("/auth/github", get(auth::oauth::github_login_handler))
            .route("/auth/github/callback", get(auth::oauth::github_callback_handler))
            .route("/auth/google", get(auth::oauth::google_login_handler))
            .route("/auth/google/callback", get(auth::oauth::google_callback_handler))
            .route("/auth/logout", post(auth::oauth::logout_handler))
            .layer(from_fn_with_state(state.clone(), middleware::rate_limit_middleware))
            .layer(from_fn(middleware::security_headers_middleware))
            .with_state(state.clone());

        // Admin tier: Essential APIs (not deprecated)
        // Public health checks (no auth required)
        let essential_public_router = Router::new()
            .route("/health", get(health_handler))
            .route("/ready", get(ready_handler))
            .route("/live", get(live_handler))
            .route("/api/v1/health", get(health_handler))
            .route("/api/v1/metrics", get(metrics_handler));

        // Authenticated essential APIs (auth required)
        let essential_auth_router = Router::new()
 // OpenAI-compatible API
            .route("/v1/chat/completions", post(openai_chat_completions_handler))
            .route("/v1/models", get(openai_list_models_handler))
 // Internal model catalog API
            .route("/api/v1/models", get(list_models_handler))
 // Computer / desktop automation API
            .route("/api/v1/reload", post(reload_all_handler))
 // Channel management API
            .route("/api/v1/channels", get(channel_list_handler))
            .route("/api/v1/channels/{name}/enable", post(enable_channel_handler))
            .route("/api/v1/channels/{name}/disable", post(disable_channel_handler))
 // Plugin management API
            .route("/api/v1/plugins", get(list_plugins_handler))
            .route("/api/v1/plugins/install", post(install_plugin_handler))
            .route("/api/v1/plugins/uninstall", post(uninstall_plugin_handler))
            .route("/api/v1/plugins/search", get(search_plugins_handler))
            .route("/api/v1/plugins/sign", post(sign_plugin_handler))
            .route("/api/v1/plugins/reload", post(reload_plugins_handler))
            .route("/api/v1/plugins/{name}/enable", post(enable_plugin_handler))
            .route("/api/v1/plugins/{name}/disable", post(disable_plugin_handler))
            .route("/api/v1/plugins/{name}/unload", delete(unload_plugin_handler))
            .route("/api/v1/plugins/{name}/reload", post(reload_plugin_handler))
 // Skill management API
            .route("/api/v1/skills", get(list_skills_handler))
            .route("/api/v1/skills/install", post(install_skill_handler))
            .route("/api/v1/skills/{name}", get(get_skill_handler))
            .route("/api/v1/skills/{name}/enable", post(enable_skill_handler))
            .route("/api/v1/skills/{name}/disable", post(disable_skill_handler))
            .route("/api/v1/skills/{name}/run", post(run_skill_handler))
            .route("/api/v1/skills/{name}/uninstall", post(uninstall_skill_handler))
 // Device pairing API
            .route("/api/v1/device/pairing/pending", get(list_device_pending_handler))
            .route("/api/v1/device/pairing/authorized", get(list_device_authorized_handler))
            .route("/api/v1/device/pairing/approve", post(approve_device_handler))
            .route("/api/v1/device/pairing/reject", post(reject_device_handler))
            .route("/api/v1/device/pairing/revoke", post(revoke_device_handler))
            .route("/api/v1/device/pairing/qr/{code}", get(device_qr_handler))
            .route("/api/v1/device/pairing/setup/{setup_code}", get(setup_device_handler))
            .layer(from_fn_with_state(state.clone(), middleware::auth_middleware));

        let essential_router = essential_public_router.merge(essential_auth_router);

        // Apply remaining middleware layers to essential routes
        // (order matters - applied in reverse)
        let admin_router = essential_router
            .layer(from_fn_with_state(state.clone(), middleware::rate_limit_middleware))
            .layer(from_fn_with_state(state.clone(), auth::session_cookie_middleware))
            .layer(from_fn_with_state(state.clone(), middleware::tailscale_auth_middleware))
            .layer(from_fn_with_state(state.clone(), middleware::trusted_proxy_auth_middleware))
            .layer(from_fn(middleware::security_headers_middleware))
            .with_state(state.clone());

        // WebSocket sub-router with mandatory auth validation middleware
        let ws_router = Router::new()
            .route("/ws", get(ws::ws_handler))
            .layer(from_fn_with_state(state.clone(), ws::ws_auth_middleware))
            .with_state(state.clone());

        // Build CORS layer from config
        let cors_layer = {
            let config = state.config.read().await;
            if config.security.cors.enabled {
                let mut cors = CorsLayer::new();
                if config.security.cors.allow_credentials {
                    cors = cors.allow_credentials(true);
                }
                // Allow configured origins
                // When allow_credentials is true, wildcard (*) is invalid CORS
                // per the Fetch spec. In that case we mirror the request origin
                // instead, which achieves the same effect for local development.
                let has_wildcard = config
                    .security
                    .cors
                    .allowed_origins
                    .iter()
                    .any(|o| o == "*");
                if has_wildcard && config.security.cors.allow_credentials {
                    cors = cors.allow_origin(tower_http::cors::AllowOrigin::mirror_request());
                } else {
                    for origin in &config.security.cors.allowed_origins {
                        if origin == "*" {
                            cors = cors.allow_origin(tower_http::cors::Any);
                        } else if let Ok(header_value) = origin.parse() {
                            cors = cors.allow_origin([header_value]);
                        }
                    }
                }
                // Allow configured methods
                let methods: Vec<_> = config
                    .security
                    .cors
                    .allowed_methods
                    .iter()
                    .filter_map(|m| m.parse().ok())
                    .collect();
                if !methods.is_empty() {
                    cors = cors.allow_methods(methods);
                }
                // Allow configured headers
                let headers: Vec<_> = config
                    .security
                    .cors
                    .allowed_headers
                    .iter()
                    .filter_map(|h| h.parse().ok())
                    .collect();
                if !headers.is_empty() {
                    cors = cors.allow_headers(headers);
                }
                cors.max_age(std::time::Duration::from_secs(
                    config.security.cors.max_age_secs as u64,
                ))
            } else {
                CorsLayer::new()
            }
        };

        // SPA frontend routes (serve built React app from embedded assets)
        let frontend_router = Router::new()
            .route("/", get(web_terminal_html_handler))
            .route("/favicon.svg", get(favicon_handler))
            .route("/syscity.png", get(syscity_png_handler))
            .route("/manifest.webmanifest", get(manifest_handler))
            .route("/registerSW.js", get(register_sw_handler))
            .route("/assets/*path", get(asset_handler));

        // Merge all routers and apply global CORS
        frontend_router
            .merge(public_router)
            .merge(auth_router)
            .merge(admin_router)
            .merge(ws_router)
            .layer(cors_layer)
    }

    /// Spawn a new agent
    async fn spawn_agent(&self, id: String, config: AgentConfig) -> crate::Result<()> {
        let handle = spawn_agent_inner(self.state.clone(), id, config).await?;
        self.spawn_agent_task(handle).await;
        Ok(())
    }
}

/// Free function that spawns an agent — callable from both `Gateway::spawn_agent`
/// and the self-repair watchdog loop. Returns the agent processing loop handle so
/// the gateway can await it during shutdown.
async fn spawn_agent_inner(
    state: Arc<GatewayState>,
    id: String,
    mut config: AgentConfig,
) -> crate::Result<JoinHandle<()>> {
    config.agent_id = Some(id.clone());
    info!("Spawning agent: {}", id);

    let (tx, mut rx) = mpsc::channel(100);

    // Create provider from model router
    let provider: Arc<dyn crate::providers::Provider> =
        state.infra.model_router.create_default_provider().await?;
    // Get tool registry from state
    let tools = state.tools.registry.clone();

    // Get the model from config for this agent
    let model = state.config.read().await.model.clone();

    // Create the actual Agent instance with model, memory manager, chat history,
    // shared cost guard, and session management stores.
    let memory_manager = state.memory.manager.read().await.as_ref().cloned();
    let cost_guard = Arc::clone(&state.agents.cost_guard);

    // Read computer config for the agent
    let computer_config = {
        let cfg = state.config.read().await;
        crate::computer::LoopConfig {
            max_steps: cfg.computer.max_steps,
            settle_delay_ms: cfg.computer.settle_delay_ms,
            ..Default::default()
        }
    };
    let computer_adapter = state.tools.computer_adapter.read().await.clone();

    // Mint a per-agent perception adapter if the perception pipeline
    // is initialized. Dispatches to the configured summarizer backend
    // (Template / Local / Llm) and respects the master enable_summary
    // switch so that the default deployment pays zero LLM tokens for
    // the periodic `### Summary` block.
    let perception_adapter: Option<Arc<dyn crate::perception::AgentPerceptionAdapter>> = {
        let init = state.perception_init.read().await;
        let p_cfg = &state.config.read().await.perception;
        // Build summarizer in the async block (not inside a sync closure)
        // because the Local variant requires async (model download).
        if let Some(p) = init.as_ref() {
            let summarizer: Option<Arc<dyn crate::perception::PerceptionSummarizer>> =
                if p_cfg.enable_summary {
                    Some(build_summarizer(
                        &p_cfg.summarizer_kind,
                        provider.clone(),
                        model.clone(),
                    ).await)
                } else {
                    None
                };
            let adapter_cfg = crate::perception::AdapterConfig {
                enable_summary: p_cfg.enable_summary,
                summary_refresh_interval: p_cfg
                    .summary_refresh_secs
                    .or(Some(60))
                    .map(std::time::Duration::from_secs),
                ..Default::default()
            };
            Some(p.context.new_adapter(
                crate::perception::Focus::default(),
                summarizer,
                adapter_cfg,
            ) as Arc<dyn crate::perception::AgentPerceptionAdapter>)
        } else {
            None
        }
    };

    let agent = if let Some(mm) = memory_manager {
        let chat_history = mm.chat_history();
        let mut builder = Agent::new(config.clone(), provider, tools)
            .with_model(model.clone())
            .with_memory_manager(mm.clone())
            .with_chat_history(chat_history)
            .with_cost_guard(cost_guard)
            .with_transcript_store(Arc::clone(&state.infra.transcript_store))
            .with_artifact_store(Arc::clone(&state.infra.artifact_store))
            .with_disk_budget(Arc::clone(&state.infra.disk_budget))
            .with_session_file_manager(Arc::clone(&state.infra.session_file_manager))
            .with_model_router(Arc::clone(&state.infra.model_router))
            .with_skill_manager(Arc::clone(&state.tools.skills_manager))
            .with_model_alias(model.clone());
        if let Some(adapter) = computer_adapter.clone() {
            builder = builder
                .with_computer_adapter(adapter)
                .with_computer_config(computer_config);
        }
        if let Some(pa) = perception_adapter.clone() {
            builder = builder.with_perception_adapter(pa);
        }
        // Attach planner state store for crash recovery on restart.
        let planner_db = crate::dirs::syscity_dir().join("planner.db");
        let url = format!("sqlite:///{}", planner_db.display());
        if let Ok(store) = crate::planner::TaskStateStore::new(&url).await {
            builder = builder.with_planner_state_store(store);
        }
        Arc::new(builder)
    } else {
        let mut builder = Agent::new(config.clone(), provider, tools)
            .with_model(model.clone())
            .with_cost_guard(cost_guard)
            .with_skill_manager(Arc::clone(&state.tools.skills_manager))
            .with_transcript_store(Arc::clone(&state.infra.transcript_store))
            .with_artifact_store(Arc::clone(&state.infra.artifact_store))
            .with_disk_budget(Arc::clone(&state.infra.disk_budget))
            .with_session_file_manager(Arc::clone(&state.infra.session_file_manager))
            .with_model_router(Arc::clone(&state.infra.model_router))
            .with_model_alias(model.clone());
        if let Some(adapter) = computer_adapter.clone() {
            builder = builder
                .with_computer_adapter(adapter)
                .with_computer_config(computer_config);
        }
        if let Some(pa) = perception_adapter.clone() {
            builder = builder.with_perception_adapter(pa);
        }
        // Attach planner state store for crash recovery on restart.
        let planner_db = crate::dirs::syscity_dir().join("planner.db");
        let url = format!("sqlite:///{}", planner_db.display());
        if let Ok(store) = crate::planner::TaskStateStore::new(&url).await {
            builder = builder.with_planner_state_store(store);
        }
        Arc::new(builder)
    };

    // Wire the new agent into the cron scheduler so routine (agent-target)
    // jobs can run. Only the first agent is wired; subsequent agents keep
    // the first one active unless explicitly overwritten.
    {
        if let Some(cron_arc) = state.scheduler.cron_scheduler.get_opt().await {
            cron_arc.lock().await.set_agent(agent.clone()).await;
            debug!("Routine engine: wired agent '{}' into cron scheduler", id);
        }
    }

    let (query_tx, mut query_rx) = mpsc::channel::<AgentQuery>(32);

    let handle = AgentHandle {
        id: id.clone(),
        config: config.clone(),
        tx: tx.clone(),
        query_tx: query_tx.clone(),
        busy: false,
        agent: agent.clone(),
    };

    {
        let mut agents = state.agents.agents.write().await;
        agents.insert(id.clone(), handle);
    }

    // Start agent processing loop
    let agent_id = id.clone();

    let task_handle = tokio::spawn(async move {
        info!("Agent {} processing loop started", agent_id);

        // Start per-agent stale-context eviction loop (check every 5 min,
        // evict contexts idle > 30 min)
        let repair_handle = agent.start_self_repair_loop(
            std::time::Duration::from_secs(300),
            std::time::Duration::from_secs(1800),
        );

        loop {
            tokio::select! {
                           cmd = rx.recv() => {
                           let cmd = match cmd { Some(c) => c, None => break };
                           match cmd {
                               AgentCommand::ProcessMessage {
                                   session_id,
                                   message,
                                   user_id,
                                   channel,
                                   model_override: _,
                               } => {
                                   let source_channel = channel;
                                   info!("Agent {} processing message for session {}", agent_id, session_id);

            // Update status to processing
                                   let _ = state.events.tx.send(GatewayEvent::AgentStatus {
                                       agent_id: agent_id.clone(),
                                       status: AgentStatus::Processing { session_id: session_id.clone() },
                                   });

            // Create incoming message for the Agent
                                   let incoming_msg = crate::channels::IncomingMessage::new(
                                       user_id.clone(),
                                       session_id.clone(),
                                       message.clone(),
                                   );

            // Build trajectory log for this turn
                                   let trajectory = Arc::new(tokio::sync::Mutex::new(
                                       crate::outbound::TrajectoryLog::new()
                                   ));
                                   {
                                       let mut traj = trajectory.lock().await;
                                       traj.push(crate::outbound::TrajectoryEntry::Start {
                                           timestamp: std::time::SystemTime::now(),
                                           session_id: session_id.clone(),
                                           agent_id: agent_id.clone(),
                                       });
                                   }

            // Create progress callback that broadcasts tool events
            // and also records trajectory entries.
                                   let progress_state = state.clone();
                                   let progress_session_id = session_id.clone();
                                   let progress_agent_id = agent_id.clone();
                                   let progress_trajectory = trajectory.clone();
                                   let progress_cb: crate::agent::ProgressCallback =
                                       Arc::new(move |event: crate::agent::ProgressEvent| {
                                           let state = progress_state.clone();
                                           let session_id = progress_session_id.clone();
                                           let agent_id = progress_agent_id.clone();
                                           let trajectory = progress_trajectory.clone();
                                           Box::pin(async move {
                                               match event {
                                                   crate::agent::ProgressEvent::ToolCalling {
                                                       name,
                                                       arguments,
                                                   } => {
                                                       info!(
                                                           "ToolCalling event: {} for session {}",
                                                           name, session_id
                                                       );
                                                       let _ =
                                                           state.events.tx.send(GatewayEvent::ToolCalling {
                                                               session_id: session_id.clone(),
                                                               agent_id: agent_id.clone(),
                                                               tool_name: name.clone(),
                                                               arguments: arguments.clone(),
                                                           });
                                                       let mut traj = trajectory.lock().await;
                                                       traj.push(crate::outbound::TrajectoryEntry::ToolCall {
                                                           timestamp: std::time::SystemTime::now(),
                                                           name: name.clone(),
                                                           arguments: serde_json::from_str(&arguments).unwrap_or(serde_json::Value::String(arguments)),
                                                       });
                                                   }
                                                   crate::agent::ProgressEvent::ToolResult {
                                                       name,
                                                       result,
                                                       data,
                                                   } => {
                                                       info!(
                                                           "ToolResult event: {} for session {}",
                                                           name, session_id
                                                       );
                                                       let _ = state.events.tx.send(GatewayEvent::ToolResult {
                                                           session_id: session_id.clone(),
                                                           agent_id: agent_id.clone(),
                                                           tool_name: name.clone(),
                                                           result: result.clone(),
                                                           data: data.clone(),
                                                       });
                                                       let mut traj = trajectory.lock().await;
                                                       traj.push(crate::outbound::TrajectoryEntry::ToolResult {
                                                           timestamp: std::time::SystemTime::now(),
                                                           name: name.clone(),
                                                           result: serde_json::from_str(&result).unwrap_or(serde_json::Value::String(result)),
                                                           duration_ms: 0, // Would need actual timing
                                                       });
                                                   }
                                                   crate::agent::ProgressEvent::Completed { response } => {
                                                       let _ = state.events.tx.send(GatewayEvent::Completed {
                                                           session_id: session_id.clone(),
                                                           agent_id: agent_id.clone(),
                                                           response,
                                                       });
                                                   }
                                                   crate::agent::ProgressEvent::Error { message } => {
                                                       let _ = state.events.tx.send(GatewayEvent::ProcessingError {
                                                           session_id: session_id.clone(),
                                                           agent_id: agent_id.clone(),
                                                           message,
                                                       });
                                                   }
                                                   _ => {}
                                               }
                                           })
                                       });

            // Process message with progress callbacks
                                   let (response_content, response_usage) = match agent
                                       .process_message_with_progress(incoming_msg, progress_cb)
                                       .await
                                   {
                                       Ok(outgoing) => {
                                           let mut traj = trajectory.lock().await;
                                           traj.push(crate::outbound::TrajectoryEntry::Finish {
                                               timestamp: std::time::SystemTime::now(),
                                               output: outgoing.content.clone(),
                                           });
                                           (outgoing.content, outgoing.usage)
                                       }
                                       Err(e) => {
                                           error!("Agent {} failed to process message: {}", agent_id, e);
                                           let mut traj = trajectory.lock().await;
                                           traj.push(crate::outbound::TrajectoryEntry::Error {
                                               timestamp: std::time::SystemTime::now(),
                                               message: e.to_string(),
                                           });
                                           (format!("Error processing message: {}", e), None)
                                       }
                                   };

            // Look up conversation_id for response routing
                                   let conversation_id = {
                                       let sessions = state.channels.session_channels.read().await;
                                       sessions
                                           .get(&session_id)
                                           .map(|(_, cid)| cid.clone())
                                           .unwrap_or_else(|| session_id.clone())
                                   };

            // Generate run_id for this agent execution (run tracking)
                                   let run_id = uuid::Uuid::new_v4().to_string();

            // Persist assistant response to session history
                                   if let Some(ref store) = state.agents.store {
                                       if let Err(e) = store
                                           .append_message(&AppendMessageParams {
                                               session_id: &session_id,
                                               role: "assistant",
                                               content: &response_content,
                                               transcript_id: Some(&session_id),
                                               run_id: Some(&run_id),
                                               ..Default::default()
                                           })
                                           .await
                                       {
                                           warn!("Failed to save assistant message to session history: {}", e);
                                       }
                                   }

            // Send response event
                                   info!("DEBUG: Agent {} sending AgentResponse for session {} (conversation: {})", agent_id, session_id, conversation_id);
                                   let _ = state.events.tx.send(GatewayEvent::AgentResponse {
                                       session_id: session_id.clone(),
                                       agent_id: agent_id.clone(),
                                       content: response_content.clone(),
                                       channel: source_channel.clone(),
                                       conversation_id: conversation_id.clone(),
                                       usage: response_usage,
                                   });

            // Extract the populated trajectory
                                   let trajectory = {
                                       let traj = trajectory.lock().await;
                                       traj.clone()
                                   };

            // Route through the outbound pipeline (trajectory → canvas → sse → reply → side effects)
                                   let outbound_ctx = crate::outbound::OutboundContext {
                                       session_id: session_id.clone(),
                                       channel: source_channel.clone(),
                                       agent_id: agent_id.clone(),
                                       raw_output: response_content,
                                       tool_calls: vec![],
                                       trajectory,
                                       usage: response_usage,
                                   };
                                   let outbound_result = state.pipelines.outbound.process(outbound_ctx).await;

            // Apply canvas updates if the pipeline produced any
                                   if let Some(canvas_update) = outbound_result.canvas_update {
                                       state.tools.canvas_manager.apply_update(&session_id, canvas_update).await;
                                   }

            // Update status to idle
                                   let _ = state.events.tx.send(GatewayEvent::AgentStatus {
                                       agent_id: agent_id.clone(),
                                       status: AgentStatus::Idle,
                                   });
                               }
                               AgentCommand::Cancel => {
                                   warn!("Agent {} received cancel command", agent_id);
                               }
                               AgentCommand::UpdateConfig(new_config) => {
                                   info!("Agent {} updating configuration", agent_id);
            // Update agent configuration dynamically
                                   {
                                       let mut agents = state.agents.agents.write().await;
                                       if let Some(handle) = agents.get_mut(&agent_id) {
                                           handle.config = new_config.clone();
                                           info!("Agent {} configuration updated", agent_id);
                                       }
                                   }
            // Send status update
                                   let _ = state.events.tx.send(GatewayEvent::AgentStatus {
                                       agent_id: agent_id.clone(),
                                       status: AgentStatus::Idle,
                                   });
                               }
                               AgentCommand::Shutdown => {
                                   info!("Agent {} shutting down", agent_id);
                                   let _ = state.events.tx.send(GatewayEvent::AgentStatus {
                                       agent_id: agent_id.clone(),
                                       status: AgentStatus::Shutdown,
                                   });
                                   break;
                               }
                           }
                           } // cmd = rx.recv() arm
                           query = query_rx.recv() => {
                               let query = match query { Some(q) => q, None => break };
                               match query {
                                   AgentQuery::GetThreadSummaries { response_tx } => {
                                       let _ = response_tx.send(agent.thread_summaries().await);
                                   }
                                   AgentQuery::GetThreadTurns { conv_id, response_tx } => {
                                       let _ = response_tx.send(agent.thread_turns_for(&conv_id).await);
                                   }
                                   AgentQuery::UndoLastTurn { conv_id, response_tx } => {
                                       let _ = response_tx.send(agent.undo_last_turn(&conv_id).await);
                                   }
                                   AgentQuery::RedoLastTurn { conv_id, response_tx } => {
                                       let _ = response_tx.send(agent.redo_last_turn(&conv_id).await);
                                   }
                                   AgentQuery::RunSkill { session_id, message, user_id, skill_trust, response_tx } => {
                                       agent.set_skill_trust(skill_trust);
                                       let incoming = crate::channels::IncomingMessage::new(
                                           user_id, &session_id, message,
                                       );
                                       let no_op: crate::agent::ProgressCallback =
                                           Arc::new(|_| Box::pin(async {}));
                                       let result =
                                           agent.process_message_with_progress(incoming, no_op).await;
                                       agent.set_skill_trust(crate::tools::SkillTrust::Trusted);
                                       let _ = response_tx.send(result);
                                   }
                               }
                           }
                       }
        } // end tokio::select! and loop

        info!("Agent {} processing loop ended", agent_id);

        // Stop the per-agent repair task when the agent exits
        repair_handle.abort();
    });

    Ok(task_handle)
}

impl Gateway {
    /// Spawn an agent from its personality (on-demand spawning)
    /// Returns true if agent was spawned, false if already exists
    pub async fn spawn_agent_from_personality(&self, agent_id: &str) -> crate::Result<bool> {
        // Check if agent already exists
        {
            let agents = self.state.agents.agents.read().await;
            if agents.contains_key(agent_id) {
                return Ok(false);
            }
        }

        // Get personality from registry
        let personality = {
            let registry = self.state.agents.registry.read().await;
            match registry.get(agent_id) {
                Some(p) => p.clone(),
                None => {
                    return Err(crate::error::SyscityError::Validation(format!(
                        "Agent '{}' not found in registry",
                        agent_id
                    )));
                }
            }
        };

        info!("🚀 On-demand spawning agent '{}' from personality", agent_id);

        // Convert personality to config
        let config = personality.to_agent_config();

        // Spawn the agent
        self.spawn_agent(agent_id.to_string(), config).await?;

        Ok(true)
    }

    /// Spawn all discovered agents
    pub async fn spawn_all_discovered_agents(&self) -> crate::Result<usize> {
        let agent_ids: Vec<String> = {
            let registry = self.state.agents.registry.read().await;
            registry.list()
        };

        let mut spawned = 0;
        for agent_id in agent_ids {
            match self.spawn_agent_from_personality(&agent_id).await {
                Ok(true) => {
                    info!("✅ Auto-spawned agent '{}'", agent_id);
                    spawned += 1;
                }
                Ok(false) => {
                    debug!("Agent '{}' already spawned, skipping", agent_id);
                }
                Err(e) => {
                    warn!("Failed to spawn agent '{}': {}", agent_id, e);
                }
            }
        }

        info!("Auto-spawned {} agents from registry", spawned);
        Ok(spawned)
    }

    /// Register a channel extension with the gateway.
    ///
    /// Extensions are wired into the inbound/outbound pipelines and replace
    /// the ad-hoc per-channel initialisation code.
    pub async fn register_channel_extension(
        &self,
        ext: Arc<dyn crate::channels::ChannelExtension>,
    ) {
        let mut registry = self.state.channels.extensions.write().await;
        registry.register(ext.clone());
        info!("Registered channel extension: {}", ext.name());
    }

    /// Get or spawn agent by ID (on-demand)
    pub async fn get_or_spawn_agent(&self, agent_id: &str) -> crate::Result<Option<AgentHandle>> {
        // First check if already spawned
        {
            let agents = self.state.agents.agents.read().await;
            if let Some(handle) = agents.get(agent_id) {
                return Ok(Some(handle.clone()));
            }
        }

        // Try to spawn from personality
        match self.spawn_agent_from_personality(agent_id).await {
            Ok(true) | Ok(false) => {
                // Now get the spawned agent
                let agents = self.state.agents.agents.read().await;
                Ok(agents.get(agent_id).cloned())
            }
            Err(e) => {
                warn!("Failed to get or spawn agent '{}': {}", agent_id, e);
                Ok(None)
            }
        }
    }

    /// Initialize configured channels
    /// Auto-connect MCP servers from config and register their tools (9.1, 9.2)
    async fn init_mcp_servers(&self) {
        let servers = &self.config.mcp.servers;
        if servers.is_empty() {
            debug!("No MCP servers configured");
            return;
        }

        info!("Auto-connecting {} configured MCP server(s)…", servers.len());

        for (server_id, server_config) in servers {
            if !server_config.auto_connect {
                info!("MCP server '{}' has auto_connect=false, skipping", server_id);
                continue;
            }

            match self
                .state.tools.mcp_manager
                .connect(server_id, server_config.clone())
                .await
            {
                Ok(tools) => {
                    info!(
                        "✅ MCP server '{}' connected: {} tool(s) discovered",
                        server_id,
                        tools.len()
                    );

                    // Register discovered tools into the ToolRegistry (9.2)
                    // ToolRegistry is behind Arc<ToolRegistry> – need interior mutability.
                    // Use Arc::get_mut if no other references, else fall back gracefully.
                    let max_tools = if server_config.max_tools == 0 {
                        tools.len()
                    } else {
                        server_config.max_tools.min(tools.len())
                    };

                    if let Some(client_arc) = self.state.tools.mcp_manager.get_client(server_id).await {
                        for tool in tools.iter().take(max_tools) {
                            let wrapper =
                                Arc::new(McpToolWrapper::new(client_arc.clone(), server_id, tool));
                            self.state.tools.registry.register_dynamic(wrapper);
                            debug!("  Registered MCP tool: mcp__{}__{}", server_id, tool.name);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to connect MCP server '{}': {}", server_id, e);
                }
            }
        }
    }

    async fn init_channels(&self) -> crate::Result<()> {
        info!("Initializing {} configured channels", self.config.channels.len());

        for (name, config) in &self.config.channels {
            if !config.enabled {
                info!("Channel {} is disabled, skipping", name);
                continue;
            }

            // Check if channel already running
            if self.state.channels.channels.read().await.contains_key(name) {
                info!("Channel {} already running, skipping", name);
                continue;
            }

            self.init_single_channel(name, config).await?;
        }

        // Discover and start WASM plugin channels
        #[cfg(feature = "plugins")]
        self.init_plugin_channels().await?;

        Ok(())
    }

    /// Discover and start WASM plugin channels.
    #[cfg(feature = "plugins")]
    async fn init_plugin_channels(&self) -> crate::Result<()> {
        use crate::channels::plugin_host::PluginChannelRegistry;
        use crate::dirs;
        

        let plugin_dir = dirs::extensions_dir().join("channels");
        if !plugin_dir.exists() {
            info!("Plugin channel directory does not exist, skipping: {:?}", plugin_dir);
            return Ok(());
        }

        // Create a shared inbound message channel for plugin channels.
        // The receiver needs to be wired into the inbound pipeline (see TODO).
        let (plugin_inbound_tx, _plugin_inbound_rx) = tokio::sync::mpsc::unbounded_channel();

        let registry = PluginChannelRegistry::new(plugin_dir, plugin_inbound_tx);
        let available = registry.discover_plugins().await?;

        if available.is_empty() {
            info!("No WASM channel plugins found");
            return Ok(());
        }

        for (name, path) in &available {
            info!("Discovered WASM channel plugin '{}' at {:?}", name, path);
        }

        for (name, _path) in &available {
            // Skip if a native channel with the same name is already running
            if self.state.channels.channels.read().await.contains_key(name) {
                info!("Channel '{}' already running as native, skipping plugin", name);
                continue;
            }

            match registry.load_plugin(name, None).await {
                Ok(plugin) => {
                    info!("Loaded WASM channel plugin '{}'", name);
                    // Register in the channel map
                    let channel: Arc<dyn crate::channels::Channel> = plugin.clone();
                    self.state.channels.channels.write()
                        .await
                        .insert(name.clone(), channel.clone());

                    // Start the plugin channel
                    if let Err(e) = plugin.start().await {
                        warn!("Failed to start WASM channel plugin '{}': {}", name, e);
                        continue;
                    }

                    // Wire health monitoring
                    if let Some(ref monitor) = self.state.channels.health_monitor {
                        let check_interval = std::time::Duration::from_secs(30);
                        let transport_timeout = std::time::Duration::from_secs(10);
                        monitor.monitor_channel_with_timeout(
                            name,
                            channel,
                            check_interval,
                            transport_timeout,
                        );
                    }

                    // Record snapshot
                    if let Some(ref store) = self.state.channels.snapshot_store {
                        let snap = crate::channels::snapshot::healthy_snapshot(name, None);
                        store.store(snap).await;
                    }

                    info!("WASM channel plugin '{}' initialized successfully", name);
                }
                Err(e) => {
                    warn!("Failed to load WASM channel plugin '{}': {}", name, e);
                }
            }
        }

        Ok(())
    }

    /// Initialize a single channel by name and config
    async fn init_single_channel(&self, name: &str, config: &ChannelConfig) -> crate::Result<()> {
        info!("Initializing channel {} ({:?})", name, config.channel_type);

        match config.channel_type {
            ChannelType::Telegram => {
                #[cfg(feature = "telegram")]
                {
                    self.init_telegram_channel(name, config).await?;
                }
                #[cfg(not(feature = "telegram"))]
                {
                    warn!("Telegram feature not enabled, skipping channel '{}'", name);
                }
            }
            ChannelType::Discord => {
                #[cfg(feature = "discord")]
                {
                    self.init_discord_channel(name, config).await?;
                }
                #[cfg(not(feature = "discord"))]
                {
                    warn!("Discord feature not enabled, skipping channel '{}'", name);
                }
            }
            ChannelType::Slack => {
                #[cfg(feature = "slack")]
                {
                    self.init_slack_channel(name, config).await?;
                }
                #[cfg(not(feature = "slack"))]
                {
                    warn!("Slack feature not enabled, skipping channel '{}'", name);
                }
            }
            ChannelType::Whatsapp => {
                #[cfg(feature = "whatsapp")]
                {
                    self.init_whatsapp_channel(name, config).await?;
                }
                #[cfg(not(feature = "whatsapp"))]
                {
                    warn!("WhatsApp feature not enabled, skipping channel '{}'", name);
                }
            }
            ChannelType::Qq => {
                #[cfg(feature = "qq")]
                {
                    self.init_qq_channel(name, config).await?;
                }
                #[cfg(not(feature = "qq"))]
                {
                    warn!("QQ feature not enabled, skipping channel '{}'", name);
                }
            }
            ChannelType::Feishu => {
                #[cfg(feature = "feishu")]
                {
                    self.init_feishu_channel(name, config).await?;
                }
                #[cfg(not(feature = "feishu"))]
                {
                    warn!("Feishu/Lark feature not enabled, skipping channel '{}'", name);
                }
            }
            ChannelType::WebTerminal => {
                info!(
                    "Channel '{}' (WebTerminal) uses Gateway WS/API directly, skipping adapter spawn",
                    name
                );
            }
            ChannelType::Websocket => {
                info!("WebSocket channel '{}' requires external connection", name);
            }
            ChannelType::Signal => {
                #[cfg(feature = "signal")]
                {
                    info!("Signal channel '{}' initialized (signal-cli daemon required)", name);
                }
                #[cfg(not(feature = "signal"))]
                {
                    warn!("Signal feature not enabled, skipping channel '{}'", name);
                }
            }
            ChannelType::Imessage => {
                #[cfg(feature = "imessage")]
                {
                    info!("iMessage channel '{}' initialized (BlueBubbles required)", name);
                }
                #[cfg(not(feature = "imessage"))]
                {
                    warn!("iMessage feature not enabled, skipping channel '{}'", name);
                }
            }
            ChannelType::Webchat => {
                #[cfg(feature = "webchat")]
                {
                    info!("WebChat channel '{}' initialized", name);
                }
                #[cfg(not(feature = "webchat"))]
                {
                    warn!("WebChat feature not enabled, skipping channel '{}'", name);
                }
            }
        }

        // Record a healthy snapshot after successful channel initialization
        if let Some(ref store) = self.state.channels.snapshot_store {
            let snap = healthy_snapshot(name, None);
            store.store(snap).await;
        }

        // Start health monitoring if configured
        if let Some(ref monitor) = self.state.channels.health_monitor {
            let channels = self.state.channels.channels.read().await;
            if let Some(channel) = channels.get(name).cloned() {
                drop(channels);
                let check_interval = std::time::Duration::from_secs(30);
                monitor.monitor_channel(name, channel, check_interval);
                info!("Started health monitoring for channel '{}'", name);
            } else {
                warn!("Channel '{}' not found in registry for health monitoring", name);
            }
        }

        Ok(())
    }

    /// Initialize Telegram channel via ChannelExtension (skeleton alignment)
    #[cfg(feature = "telegram")]
    async fn init_telegram_channel(&self, name: &str, config: &ChannelConfig) -> crate::Result<()> {
        if let Some(token) = config.credentials.get("token") {
            let telegram_config = crate::channels::telegram::TelegramConfig::new(token)
                .allow_usernames(config.allow_from.clone());

            let channel =
                Arc::new(crate::channels::telegram::TelegramChannel::new(telegram_config));

            // Agent routing is now handled by the InboundPipeline (AgentRouter)
            let agent_name = config.agent_id.as_deref().unwrap_or("default");
            info!(
                "Telegram channel '{}' will route via InboundPipeline (default agent: '{}')",
                name, agent_name
            );

            // Set channel default so the router knows which agent to use for Telegram
            self.state.agents.router
                .set_channel_default(name, agent_name.to_string(), None)
                .await;

            // Create the channel extension
            let ext = Arc::new(crate::channels::TelegramChannelExtension::new(
                channel.clone(),
                self.state.channels.session_channels.clone(),
            ));

            // Create inbound channel: extension -> inbound pipeline
            let (inbound_tx, mut inbound_rx) =
                mpsc::channel::<crate::channels::IncomingMessage>(1000);

            // Spawn extension inbound task (Telegram bot -> inbound pipeline)
            let ext_inbound = ext.clone();
            let inbound_handle = tokio::spawn(async move {
                if let Err(e) = ext_inbound.run_inbound(inbound_tx).await {
                    error!("Telegram extension inbound task failed: {}", e);
                }
            });
            self.spawn_task(inbound_handle).await;

            // Bridge inbound messages into the unified entry channel
            let state_clone = self.state.clone();
            let bridge_handle = tokio::spawn(async move {
                while let Some(message) = inbound_rx.recv().await {
                    if let Err(e) = state_clone.pipelines.inbound_entry.send(message).await {
                        warn!("Failed to submit Telegram message to inbound entry: {}", e);
                    }
                }
            });
            self.spawn_task(bridge_handle).await;

            // Create outbound channel: reply dispatcher -> extension outbound
            let (outbound_tx, outbound_rx) =
                mpsc::channel::<crate::channels::OutgoingMessage>(1000);

            // Spawn extension outbound task (outbound pipeline -> Telegram)
            let ext_outbound = ext.clone();
            let outbound_handle = tokio::spawn(async move {
                if let Err(e) = ext_outbound.run_outbound(outbound_rx).await {
                    error!("Telegram extension outbound task failed: {}", e);
                }
            });
            self.spawn_task(outbound_handle).await;

            // Register a bridge with the reply dispatcher so outbound pipeline
            // messages flow into the extension's run_outbound.
            let bridge = Arc::new(crate::channels::ChannelSenderBridge::new(name, outbound_tx));
            self.state.channels.reply_dispatcher
                .register_channel(name, bridge)
                .await;

            // Register extension in the extension registry
            self.register_channel_extension(ext).await;

            // Keep the raw channel in the channels map for direct access
            self.state.channels.channels.write()
                .await
                .insert(name.to_string(), channel);
            info!("✅ Telegram channel '{}' initialized via ChannelExtension", name);
        } else {
            warn!("Telegram channel '{}' missing 'token' in credentials", name);
        }
        Ok(())
    }

    /// Initialize Discord channel
    #[cfg(feature = "discord")]
    async fn init_discord_channel(&self, name: &str, config: &ChannelConfig) -> crate::Result<()> {
        if let Some(token) = config.credentials.get("token") {
            // Create inbound bridge: Discord message_tx -> inbound pipeline
            let (inbound_tx, mut inbound_rx) =
                mpsc::unbounded_channel::<crate::channels::IncomingMessage>();
            let mut discord_config = crate::channels::discord::DiscordConfig::new(token);
            discord_config.message_tx = Some(inbound_tx);

            let channel = Arc::new(crate::channels::discord::DiscordChannel::new(discord_config));

            // Bridge inbound messages into the unified entry channel
            let state_clone = self.state.clone();
            tokio::spawn(async move {
                while let Some(msg) = inbound_rx.recv().await {
                    if let Err(e) = state_clone.pipelines.inbound_entry.send(msg).await {
                        warn!("Failed to submit message to inbound entry: {}", e);
                    }
                }
            });

            let channel_name = name.to_string();
            let channel_for_task = channel.clone();
            tokio::spawn(async move {
                if let Err(e) = channel_for_task.start().await {
                    error!("Discord channel {} failed: {}", channel_name, e);
                }
            });
            self.state.channels.reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state.channels.channels.write()
                .await
                .insert(name.to_string(), channel);
            info!("✅ Discord channel '{}' initialized", name);
        } else {
            warn!("Discord channel '{}' missing 'token' in credentials", name);
        }
        Ok(())
    }

    /// Initialize Slack channel
    #[cfg(feature = "slack")]
    async fn init_slack_channel(&self, name: &str, config: &ChannelConfig) -> crate::Result<()> {
        if let Some(token) = config.credentials.get("token") {
            // Create inbound bridge: Slack message_tx (Socket Mode) -> inbound pipeline
            let (inbound_tx, mut inbound_rx) =
                mpsc::unbounded_channel::<crate::channels::IncomingMessage>();
            let mut slack_config = crate::channels::slack::SlackConfig::new(token);
            slack_config.message_tx = Some(inbound_tx);
            if let Some(app_token) = config.credentials.get("app_token") {
                slack_config.app_token = Some(app_token.to_string());
            }

            let channel = Arc::new(crate::channels::slack::SlackChannel::new(slack_config));

            // Bridge inbound messages into the unified entry channel
            let state_clone = self.state.clone();
            tokio::spawn(async move {
                while let Some(msg) = inbound_rx.recv().await {
                    if let Err(e) = state_clone.pipelines.inbound_entry.send(msg).await {
                        warn!("Failed to submit message to inbound entry: {}", e);
                    }
                }
            });

            let channel_name = name.to_string();
            let channel_for_task = channel.clone();
            tokio::spawn(async move {
                if let Err(e) = channel_for_task.start().await {
                    error!("Slack channel {} failed: {}", channel_name, e);
                }
            });
            self.state.channels.reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state.channels.channels.write()
                .await
                .insert(name.to_string(), channel);
            info!("✅ Slack channel '{}' initialized", name);
        } else {
            warn!("Slack channel '{}' missing 'token' in credentials", name);
        }
        Ok(())
    }

    /// Initialize WhatsApp channel
    #[cfg(feature = "whatsapp")]
    async fn init_whatsapp_channel(&self, name: &str, config: &ChannelConfig) -> crate::Result<()> {
        if let (Some(phone_id), Some(token)) = (
            config.credentials.get("phone_number_id"),
            config.credentials.get("access_token"),
        ) {
            let whatsapp_config = crate::channels::whatsapp::WhatsappConfig::new(phone_id, token);

            let channel =
                Arc::new(crate::channels::whatsapp::WhatsappChannel::new(whatsapp_config));
            let channel_name = name.to_string();
            let channel_for_task = channel.clone();
            tokio::spawn(async move {
                if let Err(e) = channel_for_task.start().await {
                    error!("WhatsApp channel {} failed: {}", channel_name, e);
                }
            });
            self.state.channels.reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state.channels.channels.write()
                .await
                .insert(name.to_string(), channel);
            info!("✅ WhatsApp channel '{}' initialized", name);
        } else {
            warn!(
                "WhatsApp channel '{}' missing 'phone_number_id' or 'access_token' in credentials",
                name
            );
        }
        Ok(())
    }

    /// Initialize Feishu/Lark channel (outbound via ReplyDispatcher)
    #[cfg(feature = "feishu")]
    async fn init_feishu_channel(&self, name: &str, config: &ChannelConfig) -> crate::Result<()> {
        if let (Some(app_id), Some(app_secret)) =
            (config.credentials.get("app_id"), config.credentials.get("app_secret"))
        {
            let lark_config = crate::channels::lark::LarkConfig::new(app_id, app_secret);

            let channel = Arc::new(crate::channels::lark::LarkChannel::new(lark_config));
            let channel_name = name.to_string();
            let channel_for_task = channel.clone();
            tokio::spawn(async move {
                if let Err(e) = channel_for_task.start().await {
                    error!("Feishu channel {} failed: {}", channel_name, e);
                }
            });
            self.state.channels.reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state.channels.channels.write()
                .await
                .insert(name.to_string(), channel);
            info!("✅ Feishu channel '{}' initialized (inbound via webhook)", name);
        } else {
            warn!("Feishu channel '{}' missing 'app_id' or 'app_secret' in credentials", name);
        }
        Ok(())
    }

    /// Initialize QQ channel
    #[cfg(feature = "qq")]
    async fn init_qq_channel(&self, name: &str, config: &ChannelConfig) -> crate::Result<()> {
        if let (Some(app_id), Some(app_secret), Some(bot_qq)) = (
            config.credentials.get("app_id"),
            config.credentials.get("app_secret"),
            config.credentials.get("bot_qq"),
        ) {
            // Create inbound bridge: QQ WebSocket -> inbound pipeline
            let (inbound_tx, mut inbound_rx) =
                mpsc::unbounded_channel::<crate::channels::IncomingMessage>();
            let mut qq_config = crate::channels::qq::QqConfig::new(app_id, app_secret, bot_qq);
            qq_config.message_tx = Some(inbound_tx);

            let channel = Arc::new(crate::channels::qq::QqChannel::new(qq_config));

            // Bridge inbound messages into the unified entry channel
            let state_clone = self.state.clone();
            tokio::spawn(async move {
                while let Some(msg) = inbound_rx.recv().await {
                    if let Err(e) = state_clone.pipelines.inbound_entry.send(msg).await {
                        warn!("Failed to submit message to inbound entry: {}", e);
                    }
                }
            });

            let channel_name = name.to_string();
            let channel_for_task = channel.clone();
            tokio::spawn(async move {
                if let Err(e) = channel_for_task.start().await {
                    error!("QQ channel {} failed: {}", channel_name, e);
                }
            });
            self.state.channels.reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state.channels.channels.write()
                .await
                .insert(name.to_string(), channel);
            info!("✅ QQ channel '{}' initialized", name);
        } else {
            warn!(
                "QQ channel '{}' missing required credentials (app_id, app_secret, bot_qq)",
                name
            );
        }
        Ok(())
    }

    /// Unified worker that consumes `IncomingMessage`s from `inbound_entry`
    /// and drives them through the inbound pipeline.
    ///
    /// The pipeline forwards `RoutedMessage`s to `routed_tx`; the separate
    /// `process_routed_messages` worker handles actual agent dispatch.
    async fn process_inbound_entries(
        state: Arc<GatewayState>,
        mut rx: mpsc::Receiver<crate::channels::IncomingMessage>,
        shutdown: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("Inbound entry worker received shutdown signal");
                    break;
                }
                Some(incoming) = rx.recv() => {
                    match state.pipelines.inbound.process(incoming).await {
                        Some(routed) => {
                            info!("Inbound message routed through pipeline: agent={}", routed.agent_id);
                        }
                        None => {
                            info!("Inbound message absorbed by pipeline (debounced or suppressed)");
                        }
                    }
                }
            }
        }
    }

    /// Process routed messages from the inbound pipeline.
    ///
    /// Converts `RoutedMessage` into `AgentCommand::ProcessMessage` and
    /// forwards it to the resolved agent, respecting `QueueMode`.
    async fn process_routed_messages(
        state: Arc<GatewayState>,
        mut rx: mpsc::Receiver<crate::inbound::RoutedMessage>,
        shutdown: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("Routed message worker received shutdown signal");
                    break;
                }
                Some(routed) = rx.recv() => {
                    Self::dispatch_routed_message(&state, routed).await;
                }
            }
        }
    }

    /// Dispatch a single `RoutedMessage` to the resolved agent.
    async fn dispatch_routed_message(state: &Arc<GatewayState>, routed: crate::inbound::RoutedMessage) {
        if routed.suppress_delivery {
            debug!("Suppressing delivery for session {}", routed.incoming.conversation_id.0);
            return;
        }

        let session_id = routed.incoming.conversation_id.0.clone();
        let agent_id = routed.agent_id.clone();
        let channel = match &routed.incoming.provenance {
            crate::channels::InputProvenance::ExternalUser { channel, .. } => channel.clone(),
            _ => "unknown".to_string(),
        };

        // ── Group session membership check ───────────────────────────────
        {
            let user_id = &routed.incoming.user_id.0;
            let groups = state.agents.group_manager.read().await;
            if let Some(group) = groups.get_group(&session_id) {
                let group = group.read().await;
                if !group.is_member(user_id) {
                    warn!(
                        "User {} is not a member of group session {}, dropping message",
                        user_id, session_id
                    );
                    return;
                }
                if let Some(member) = group.get_member(user_id) {
                    if !member.role.can_participate() {
                        warn!(
                            "User {} (role: {}) cannot participate in group session {}, dropping message",
                            user_id, member.role, session_id
                        );
                        return;
                    }
                }
            }
        }

        match routed.queue_mode {
            crate::inbound::QueueMode::Interrupt => {
                // Clear any buffered messages for this session
                {
                    let mut buffers = state.agents.message_buffer.write().await;
                    buffers.remove(&session_id);
                }
                Self::send_to_agent(
                    state,
                    &agent_id,
                    &session_id,
                    &routed.incoming.content,
                    &routed.incoming.user_id.0,
                    &channel,
                )
                .await;
            }

            crate::inbound::QueueMode::Steer => {
                // Send Cancel to agent (best-effort), then send the steer message
                {
                    let agents = state.agents.agents.read().await;
                    if let Some(agent) = agents.get(&agent_id) {
                        let _ = agent.tx.send(AgentCommand::Cancel).await;
                    }
                }
                // Small delay to let cancel take effect
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                Self::send_to_agent(
                    state,
                    &agent_id,
                    &session_id,
                    &routed.incoming.content,
                    &routed.incoming.user_id.0,
                    &channel,
                )
                .await;
            }

            crate::inbound::QueueMode::FollowUp => {
                // Buffer message; flush after a delay if no more arrive
                let should_flush = {
                    let mut buffers = state.agents.message_buffer.write().await;
                    let buffer = buffers.entry(session_id.clone()).or_default();
                    buffer.push(BufferedMessage {
                        content: routed.incoming.content.clone(),
                        user_id: routed.incoming.user_id.0.clone(),
                        channel: channel.clone(),
                    });
                    buffer.len() >= 5 // Max 5 messages before forced flush
                };

                if should_flush {
                    Self::flush_session_buffer(state, &agent_id, &session_id).await;
                } else {
                    // Spawn a delayed flush task
                    let state_clone = state.clone();
                    let agent_id_clone = agent_id.clone();
                    let session_id_clone = session_id.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                        Self::flush_session_buffer(
                            &state_clone,
                            &agent_id_clone,
                            &session_id_clone,
                        )
                        .await;
                    });
                }
            }

            crate::inbound::QueueMode::Collect => {
                // /done trigger: flush the buffer
                let has_buffered = {
                    let buffers = state.agents.message_buffer.read().await;
                    buffers
                        .get(&session_id)
                        .map(|b| !b.is_empty())
                        .unwrap_or(false)
                };

                if has_buffered {
                    Self::flush_session_buffer(state, &agent_id, &session_id).await;
                } else {
                    // No buffer to flush; treat as normal message
                    Self::send_to_agent(
                        state,
                        &agent_id,
                        &session_id,
                        &routed.incoming.content,
                        &routed.incoming.user_id.0,
                        &channel,
                    )
                    .await;
                }
            }

            crate::inbound::QueueMode::Normal => {
                Self::send_to_agent(
                    state,
                    &agent_id,
                    &session_id,
                    &routed.incoming.content,
                    &routed.incoming.user_id.0,
                    &channel,
                )
                .await;
            }
        }
    }

    /// Flush buffered messages for a session and send as a single batch.
    async fn flush_session_buffer(state: &Arc<GatewayState>, agent_id: &str, session_id: &str) {
        let messages: Vec<BufferedMessage> = {
            let mut buffers = state.agents.message_buffer.write().await;
            buffers.remove(session_id).unwrap_or_default()
        };

        if messages.is_empty() {
            return;
        }

        let combined = messages
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n\n");

        let first_user_id = messages
            .first()
            .map(|m| m.user_id.clone())
            .unwrap_or_default();
        let first_channel = messages
            .first()
            .map(|m| m.channel.clone())
            .unwrap_or_default();

        info!(
            "Flushing {} buffered messages for session {} (combined length: {})",
            messages.len(),
            session_id,
            combined.len()
        );

        Self::send_to_agent(state, agent_id, session_id, &combined, &first_user_id, &first_channel)
            .await;
    }

    /// Extract a concise session name from the first assistant response.
    /// Strips markdown, takes the first meaningful words, and limits length.
    fn extract_session_name(content: &str) -> String {
        // Strip common markdown patterns
        let cleaned = content
            .replace("#", "")
            .replace("**", "")
            .replace("*", "")
            .replace("`", "")
            .replace(">", "")
            .replace("-", "")
            .replace("|", "")
            .replace("\n", " ")
            .replace("\r", " ");

        let name = cleaned
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ");

        if name.len() > 40 {
            format!("{}...", &name[..40])
        } else if name.is_empty() {
            "New Session".to_string()
        } else {
            name
        }
    }

    /// Send a single message to an agent via the ACP controller.
    ///
    /// This routes execution through the centralized ACP actor queue,
    /// enabling per-session serial processing and runtime controls
    /// (pause / resume / step / cancel).
    async fn send_to_agent(
        state: &Arc<GatewayState>,
        agent_id: &str,
        session_id: &str,
        message: &str,
        user_id: &str,
        channel: &str,
    ) {
        let agents = state.agents.agents.read().await;
        let agent_handle = match agents.get(agent_id) {
            Some(h) => h.clone(),
            None => {
                error!("Agent {} not found for session {}", agent_id, session_id);
                return;
            }
        };
        drop(agents);

        // Apply thinking config from runtime settings
        let think_level = {
            let s = state.infra.runtime_settings.read().await;
            s.get("think.level")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        let extra = think_level.and_then(|level| {
            let budget = match level.as_str() {
                "minimal" => 1024u32,
                "low" => 4096u32,
                "medium" => 16000u32,
                "high" => 32000u32,
                _ => return None,
            };
            Some(serde_json::json!({ "thinking": { "type": "enabled", "budget_tokens": budget } }))
        });
        agent_handle.agent.set_extra_params(extra).await;

        // Check queue mode and apply interrupt behavior if needed
        let queue_mode = {
            let s = state.infra.runtime_settings.read().await;
            s.get("queue.mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        if queue_mode.as_deref() == Some("interrupt") {
            state.agents.acp.cancel(session_id.to_string()).await;
        }

        let incoming_msg = crate::channels::IncomingMessage::new(
            user_id.to_string(),
            session_id.to_string(),
            message.to_string(),
        )
        .with_provenance(crate::channels::InputProvenance::ExternalUser {
            channel: channel.to_string(),
            is_direct: true,
        });

        // Broadcast processing status
        let _ = state.events.tx.send(GatewayEvent::AgentStatus {
            agent_id: agent_id.to_string(),
            status: AgentStatus::Processing {
                session_id: session_id.to_string(),
            },
        });

        // Build progress callback that forwards events to gateway subscribers
        let event_tx = state.events.tx.clone();
        let runtime_settings = state.infra.runtime_settings.clone();
        let progress_session = session_id.to_string();
        let progress_agent = agent_id.to_string();
        let progress_channel = channel.to_string();
        let progress_cb: crate::agent::ProgressCallback = Arc::new(move |event| {
            let tx = event_tx.clone();
            let settings = runtime_settings.clone();
            let sid = progress_session.clone();
            let aid = progress_agent.clone();
            let _ch = progress_channel.clone();
            Box::pin(async move {
                // Read directive settings
                let reasoning_vis = {
                    let s = settings.read().await;
                    s.get("reasoning.visibility")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                };
                let verbose_mode = {
                    let s = settings.read().await;
                    s.get("verbose.mode")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                };
                match event {
                    crate::agent::ProgressEvent::Started => {
                        let _ = tx.send(GatewayEvent::AgentStatus {
                            agent_id: aid.clone(),
                            status: AgentStatus::Processing { session_id: sid.clone() },
                        });
                    }
                    crate::agent::ProgressEvent::Generating { content } => {
                        // Skip reasoning events if visibility is off
                        if reasoning_vis.as_deref() == Some("off") {
                            return;
                        }
                        // Only emit thinking events when there's actual content
                        if let Some(ref thinking) = content {
                            if !thinking.is_empty() {
                                let _ = tx.send(GatewayEvent::Thinking {
                                    session_id: sid.clone(),
                                    agent_id: aid.clone(),
                                    content: Some(thinking.clone()),
                                });
                            }
                        }
                    }
                    crate::agent::ProgressEvent::ContentDelta { text } => {
                        let _ = tx.send(GatewayEvent::ContentDelta {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            delta: text,
                        });
                    }
                    crate::agent::ProgressEvent::ToolCalling { name, arguments } => {
                        // Skip tool events if verbose is off
                        if verbose_mode.as_deref() == Some("off") {
                            return;
                        }
                        let _ = tx.send(GatewayEvent::ToolCalling {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            tool_name: name.clone(),
                            arguments: arguments.clone(),
                        });
                    }
                    crate::agent::ProgressEvent::ToolResult { name, result, data } => {
                        // Skip tool events if verbose is off
                        if verbose_mode.as_deref() == Some("off") {
                            return;
                        }
                        // In compact verbose mode, truncate long results
                        let result = if verbose_mode.as_deref() == Some("compact") {
                            if result.len() > 500 {
                                format!("{}... (truncated)", &result[..500])
                            } else {
                                result
                            }
                        } else {
                            result
                        };
                        let _ = tx.send(GatewayEvent::ToolResult {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            tool_name: name.clone(),
                            result,
                            data,
                        });
                    }
                    crate::agent::ProgressEvent::ToolResultDelta { .. } => {
                        // Streaming tool chunks are accumulated locally and emitted
                        // as a final ToolResult event; no per-chunk gateway event yet.
                    }
                    crate::agent::ProgressEvent::Completed { response } => {
                        let _ = tx.send(GatewayEvent::Completed {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            response,
                        });
                    }
                    crate::agent::ProgressEvent::Error { message } => {
                        let _ = tx.send(GatewayEvent::ProcessingError {
                            session_id: sid.clone(),
                            agent_id: aid.clone(),
                            message,
                        });
                    }
                }
            })
        });

        // Route through ACP for serialized execution
        match state.agents.acp
            .execute_session_with_progress(agent_handle.agent.clone(), incoming_msg, progress_cb)
            .await
        {
            Ok(mut outgoing) => {
                // Apply reasoning visibility filter
                let reasoning_vis = {
                    let s = state.infra.runtime_settings.read().await;
                    s.get("reasoning.visibility")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                };
                if reasoning_vis.as_deref() == Some("off") {
                    outgoing.reasoning_content = None;
                }

                // Accumulate usage statistics
                if let Some(ref usage) = outgoing.usage {
                    let mut settings = state.infra.runtime_settings.write().await;
                    let current_tokens = settings
                        .get("usage.tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let total_tokens = usage.prompt_tokens as u64 + usage.completion_tokens as u64;
                    settings.insert(
                        "usage.tokens".to_string(),
                        serde_json::json!(current_tokens + total_tokens),
                    );
                    let current_calls = settings
                        .get("usage.calls")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let tool_calls = outgoing
                        .tool_calls
                        .as_ref()
                        .map(|c| c.len() as u64)
                        .unwrap_or(0);
                    settings.insert(
                        "usage.calls".to_string(),
                        serde_json::json!(current_calls + tool_calls + 1),
                    );
                }

                // Generate run_id for this agent execution (run tracking)
                let run_id = uuid::Uuid::new_v4().to_string();

                // Save assistant response to persistent session history
                if let Some(ref store) = state.agents.store {
                    let reasoning = outgoing.reasoning_content.as_deref();
                    let tool_calls_json = outgoing
                        .tool_calls
                        .as_ref()
                        .map(|calls| serde_json::to_string(calls).unwrap_or_default());
                    if let Err(e) = store
                        .append_message(&AppendMessageParams {
                            session_id,
                            role: "assistant",
                            content: &outgoing.content,
                            reasoning_content: reasoning,
                            tool_calls_json: tool_calls_json.as_deref(),
                            transcript_id: Some(session_id),
                            run_id: Some(&run_id),
                            ..Default::default()
                        })
                        .await
                    {
                        warn!("Failed to save assistant message to session history: {}", e);
                    }

                    // Auto-name session from first assistant response if no name yet
                    if let Ok(existing) = store.get_session_name(session_id).await {
                        if existing.is_none() {
                            let name = Self::extract_session_name(&outgoing.content);
                            if let Err(e) = store.set_session_name(session_id, &name).await {
                                warn!("Failed to save session name: {}", e);
                            } else {
                                info!("Session {} auto-named: '{}'", session_id, name);
                                let _ = state.events.tx.send(GatewayEvent::SessionRenamed {
                                    session_id: session_id.to_string(),
                                    name: name.clone(),
                                });
                            }
                        }
                    }
                }
                let _ = state.events.tx.send(GatewayEvent::AgentResponse {
                    session_id: session_id.to_string(),
                    agent_id: agent_id.to_string(),
                    content: outgoing.content,
                    channel: channel.to_string(),
                    conversation_id: session_id.to_string(),
                    usage: outgoing.usage,
                });
            }
            Err(e) => {
                error!("ACP execution failed for agent {} session {}: {}", agent_id, session_id, e);
                let _ = state.events.tx.send(GatewayEvent::ProcessingError {
                    session_id: session_id.to_string(),
                    agent_id: agent_id.to_string(),
                    message: format!("Execution failed: {}", e),
                });
            }
        }

        let _ = state.events.tx.send(GatewayEvent::AgentStatus {
            agent_id: agent_id.to_string(),
            status: AgentStatus::Idle,
        });
    }

    /// Start Tailscale for remote access.
    #[cfg(feature = "tailscale")]
    async fn start_tailscale(&self) -> crate::Result<()> {
        info!("Starting Tailscale integration...");
        crate::tailscale::start(self.config.port, self.config.tailscale_domain.clone()).await?;
        Ok(())
    }

    /// Register hot reload handlers for config changes
    async fn register_hot_reload_handlers(&self, hot_reload: &HotReloadManager) {
        use crate::config::hot_reload::ConfigFileType;

        let state = self.state.clone();
        let current_config = self.config.clone();

        // Pre-clone for handlers registered after the main handler
        // (the main handler's `move` closure will consume `state` and `current_config`)
        let state_agent = state.clone();
        let state_channel = state.clone();
        let current_config_channel = current_config.clone();
        let state_plugin = state.clone();
        let state_gateway = state.clone();

        // Handler for main config changes (includes syscity.toml)
        hot_reload
            .register_handler(ConfigFileType::Main, move |_event| {
                let state = state.clone();
                let current_config = current_config.clone();
                async move {
                    info!("Main config file changed - reloading configuration");

                    // Reload config from disk
                    let config_path = crate::dirs::syscity_dir().join("syscity.toml");
                    if !config_path.exists() {
                        return Ok(());
                    }

                    let content = match tokio::fs::read_to_string(&config_path).await {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to read syscity.toml: {}", e);
                            return Ok(());
                        }
                    };

                    let new_config: GatewayConfig = match toml::from_str(&content) {
                        Ok(cfg) => cfg,
                        Err(e) => {
                            error!("Failed to parse syscity.toml: {}", e);
                            return Ok(());
                        }
                    };

                    // Get current running channels
                    let current_channels: Vec<String> = {
                        let channels = state.channels.channels.read().await;
                        channels.keys().cloned().collect()
                    };

                    // 1. Stop removed or disabled channels
                    for name in &current_channels {
                        let should_stop = match new_config.channels.get(name) {
                            None => {
                                info!("Channel '{}' removed from config, stopping...", name);
                                true
                            }
                            Some(cfg) if !cfg.enabled => {
                                info!("Channel '{}' disabled in config, stopping...", name);
                                true
                            }
                            _ => false,
                        };

                        if should_stop {
                            // Remove channel from state (channel will be dropped, should clean up itself)
                            let removed = {
                                let mut channels = state.channels.channels.write().await;
                                channels.remove(name).is_some()
                            };
                            if removed {
                                info!("✅ Stopped channel '{}'", name);
                            }
                        }
                    }

                    // 2. Handle new or modified channels
                    for (name, new_channel_config) in &new_config.channels {
                        if !new_channel_config.enabled {
                            continue;
                        }

                        let existing = {
                            let channels = state.channels.channels.read().await;
                            channels.get(name).cloned()
                        };

                        match existing {
                            Some(_channel) => {
                                // Channel exists - check if config changed
                                let old_config = current_config.channels.get(name);
                                let config_changed = old_config
                                    .map(|old| {
                                        old.credentials != new_channel_config.credentials
                                            || old.allow_from != new_channel_config.allow_from
                                            || old.block_from != new_channel_config.block_from
                                            || old.dm_policy != new_channel_config.dm_policy
                                    })
                                    .unwrap_or(true);

                                if config_changed {
                                    info!("Channel '{}' config changed, restarting...", name);

                                    // Remove old channel
                                    {
                                        let mut channels = state.channels.channels.write().await;
                                        channels.remove(name);
                                    }

                                    // Start with new config
                                    let gateway = Gateway {
                                        state: state.clone(),
                                        config: new_config.clone(),
                                        shutdown_token: CancellationToken::new(),
                                        background_tasks: tokio::sync::Mutex::new(Vec::new()),
                                        message_workers: tokio::sync::Mutex::new(Vec::new()),
                                        agent_tasks: tokio::sync::Mutex::new(Vec::new()),
                                        device_registry: None,
                                        perception_registry: None,
                                    };
                                    if let Err(e) =
                                        gateway.init_single_channel(name, new_channel_config).await
                                    {
                                        error!("Failed to restart channel '{}': {}", name, e);
                                    } else {
                                        info!("✅ Restarted channel '{}' with new config", name);
                                    }
                                }
                            }
                            None => {
                                // New channel - initialize it
                                info!(
                                    "Hot-reloading new channel: {} ({:?})",
                                    name, new_channel_config.channel_type
                                );

                                let gateway = Gateway {
                                    state: state.clone(),
                                    config: new_config.clone(),
                                    shutdown_token: CancellationToken::new(),
                                    background_tasks: tokio::sync::Mutex::new(Vec::new()),
                                    message_workers: tokio::sync::Mutex::new(Vec::new()),
                                    agent_tasks: tokio::sync::Mutex::new(Vec::new()),
                                    device_registry: None,
                                    perception_registry: None,
                                };
                                if let Err(e) =
                                    gateway.init_single_channel(name, new_channel_config).await
                                {
                                    error!("Failed to hot-reload channel '{}': {}", name, e);
                                } else {
                                    info!("✅ Hot-reloaded channel '{}'", name);
                                }
                            }
                        }
                    }

                    Ok(())
                }
            })
            .await;

        // Handler for agent config changes
        {
            let state = state_agent;
            hot_reload
                .register_handler(ConfigFileType::Agent, move |event| {
                    let state = state.clone();
                    async move {
                        let agent_name = event
                            .path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        info!("Agent config changed for '{}': {:?}", agent_name, event.path);

                        let content = match tokio::fs::read_to_string(&event.path).await {
                            Ok(c) => c,
                            Err(e) => {
                                error!("Failed to read agent config {:?}: {}", event.path, e);
                                return Ok(());
                            }
                        };

                        let new_config: AgentConfig = match toml::from_str(&content) {
                            Ok(c) => c,
                            Err(e) => {
                                error!("Failed to parse agent config for '{}': {}", agent_name, e);
                                return Ok(());
                            }
                        };

                        // Send UpdateConfig to the running agent if it exists
                        let agents = state.agents.agents.read().await;
                        if let Some(handle) = agents.get(&agent_name) {
                            match handle.tx.send(AgentCommand::UpdateConfig(new_config)).await {
                                Ok(_) => {
                                    info!("✅ Sent config update to agent '{}'", agent_name)
                                }
                                Err(e) => warn!(
                                    "Failed to send config update to agent '{}': {}",
                                    agent_name, e
                                ),
                            }
                        } else {
                            info!(
                                "Agent '{}' not currently running; config will apply on next start",
                                agent_name
                            );
                        }

                        Ok(())
                    }
                })
                .await;
        }

        // Handler for channel config changes
        {
            let state = state_channel;
            let current_config = current_config_channel;
            hot_reload
                .register_handler(ConfigFileType::Channel, move |event| {
                    let state = state.clone();
                    let current_config = current_config.clone();
                    async move {
                        let channel_name = event
                            .path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        info!("Channel config changed for '{}': {:?}", channel_name, event.path);

                        let content = match tokio::fs::read_to_string(&event.path).await {
                            Ok(c) => c,
                            Err(e) => {
                                error!("Failed to read channel config {:?}: {}", event.path, e);
                                return Ok(());
                            }
                        };

                        let new_channel_config: ChannelConfig = match toml::from_str(&content) {
                            Ok(c) => c,
                            Err(e) => {
                                error!(
                                    "Failed to parse channel config for '{}': {}",
                                    channel_name, e
                                );
                                return Ok(());
                            }
                        };

                        if !new_channel_config.enabled {
                            let mut channels = state.channels.channels.write().await;
                            if channels.remove(&channel_name).is_some() {
                                info!("✅ Stopped disabled channel '{}'", channel_name);
                            }
                            return Ok(());
                        }

                        // Stop existing channel
                        {
                            let mut channels = state.channels.channels.write().await;
                            channels.remove(&channel_name);
                        }

                        // Re-initialize with new config
                        let gateway = Gateway {
                            state: state.clone(),
                            config: current_config.clone(),
                            shutdown_token: CancellationToken::new(),
                            background_tasks: tokio::sync::Mutex::new(Vec::new()),
                            message_workers: tokio::sync::Mutex::new(Vec::new()),
                            agent_tasks: tokio::sync::Mutex::new(Vec::new()),
                            device_registry: None,
                            perception_registry: None,
                        };
                        match gateway
                            .init_single_channel(&channel_name, &new_channel_config)
                            .await
                        {
                            Ok(_) => info!(
                                "✅ Hot-reloaded channel '{}' with updated config",
                                channel_name
                            ),
                            Err(e) => {
                                error!("Failed to hot-reload channel '{}': {}", channel_name, e)
                            }
                        }

                        Ok(())
                    }
                })
                .await;
        }

        // Handler for plugin config changes
        {
            let state = state_plugin;
            hot_reload
                .register_handler(ConfigFileType::Plugin, move |event| {
                    let state = state.clone();
                    async move {
                        let plugin_dir = event.path.parent().unwrap_or(&event.path).to_path_buf();
                        let plugin_id = plugin_dir
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or_default()
                            .to_string();

                        info!("Plugin file changed for '{}': {:?}", plugin_id, event.path);

                        if plugin_id.is_empty() {
                            warn!("Could not determine plugin ID from path {:?}", event.path);
                            return Ok(());
                        }

 // Try state-preserving reload first
                        match state.infra.plugin_manager.reload_plugin(&plugin_id).await {
                            Ok(reloaded_id) => {
                                info!("✅ Reloaded plugin '{}' (preserved state)", reloaded_id);
                            }
                            Err(e) => {
                                warn!(
                                    "State-preserving reload failed for '{}', falling back to unload+load: {}",
                                    plugin_id, e
                                );
                                match state.infra.plugin_manager.unload_plugin(&plugin_id).await {
                                    Ok(true) => {
                                        match state.infra.plugin_manager.load_plugin(&plugin_dir).await
                                        {
                                            Ok(loaded_id) => {
                                                info!(
                                                    "✅ Reloaded plugin '{}' (id={})",
                                                    plugin_id, loaded_id
                                                )
                                            }
                                            Err(e) => {
                                                error!(
                                                    "Failed to reload plugin '{}': {}",
                                                    plugin_id, e
                                                )
                                            }
                                        }
                                    }
                                    Ok(false) => {
                                        match state.infra.plugin_manager.load_plugin(&plugin_dir).await
                                        {
                                            Ok(loaded_id) => {
                                                info!(
                                                    "✅ Loaded new plugin '{}' (id={})",
                                                    plugin_id, loaded_id
                                                )
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "Could not load plugin '{}': {}",
                                                    plugin_id, e
                                                )
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to unload plugin '{}': {}", plugin_id, e)
                                    }
                                }
                            }
                        }

                        Ok(())
                    }
                })
                .await;
        }

        // Handler for gateway config changes
        {
            let state = state_gateway;
            hot_reload
                .register_handler(ConfigFileType::Gateway, move |event| {
                    let state = state.clone();
                    async move {
                        info!("Gateway config changed: {:?}", event.path);

                        // Snapshot before applying changes
                        let pre_snapshot = state.config.read().await.snapshot();

                        let content = match tokio::fs::read_to_string(&event.path).await {
                            Ok(c) => c,
                            Err(e) => {
                                error!("Failed to read gateway config {:?}: {}", event.path, e);
                                return Ok(());
                            }
                        };

                        let new_config: GatewayConfig = match toml::from_str(&content) {
                            Ok(c) => c,
                            Err(e) => {
                                error!("Failed to parse gateway config: {}", e);
                                return Ok(());
                            }
                        };

                        // Apply hot-reloadable fields (those that don't require server restart)
                        let mut config = state.config.write().await;
                        config.security = new_config.security;
                        config.providers = new_config.providers;
                        config.mcp = new_config.mcp;
                        config.hot_reload = new_config.hot_reload;
                        drop(config);
                        info!(
                            "✅ Applied gateway config updates (security, providers, mcp settings)"
                        );

                        // Compute diff and log to audit
                        let post_config = state.config.read().await;
                        let changes = post_config.diff_since(&pre_snapshot);
                        drop(post_config);

                        if !changes.is_empty() {
                            let details = serde_json::to_value(&changes).unwrap_or_default();
                            state.auth.audit_log
                                .log(
                                    crate::security::runtime_audit::AuditEventType::ConfigChange,
                                    "file_watcher",
                                    "config",
                                    true,
                                    format!("Hot-reload config change: {} field(s)", changes.len()),
                                    Some(details),
                                )
                                .await;
                            info!(
                                changes = ?changes.iter().map(|c| &c.path).collect::<Vec<_>>(),
                                "Gateway config hot-reload changes"
                            );
                        }

                        Ok(())
                    }
                })
                .await;
        }

        info!("Registered hot reload handlers for all config types");
    }
}

/// Create default tool registry with all built-in tools
#[allow(clippy::too_many_arguments)]
async fn create_default_tool_registry(
    acp: Arc<AcpControlPlane>,
    mcp_manager: Arc<McpManager>,
    approval_queue: Arc<ApprovalQueue>,
    session_store: Option<Arc<crate::agent::session_store::SessionStore>>,
    memory_manager: Arc<tokio::sync::RwLock<Option<Arc<crate::memory::MemoryManager>>>>,
    capabilities: crate::config::CapabilitiesConfig,
    audit_log: Arc<dyn crate::security::runtime_audit::AuditLogger>,
    content_filter: Option<Arc<crate::security::content_filter::ContentFilter>>,
) -> crate::Result<ToolRegistry> {
    use crate::tools::*;

    let mut registry = ToolRegistry::new()
        .with_approval_queue(approval_queue)
        .with_audit_log(audit_log);
    if let Some(filter) = content_filter {
        registry = registry.with_content_filter(filter);
    }

    // Register file system tools
    registry.register(Box::new(FileReadTool::new()));
    registry.register(Box::new(FileWriteTool::new()));
    registry.register(Box::new(FileEditTool::new()));
    registry.register(Box::new(GlobTool::new()));
    registry.register(Box::new(GrepTool::new()));

    // Register shell/execution tools wrapped in sandbox for path & timeout enforcement.
    // ShellTool needs network access (git, curl, etc.); CodeExecutionTool does not.
    registry.register(Box::new(SandboxedTool::new(
        ShellTool::new(),
        SandboxConfig {
            allow_network_access: true,
            ..SandboxConfig::default()
        },
    )));
    registry.register(Box::new(SandboxedTool::new(
        CodeExecutionTool::default(),
        SandboxConfig::default(),
    )));

    // Register web tools
    registry.register(Box::new(WebSearchTool::new()));
    registry.register(Box::new(WebFetchTool::new()));

    // Register todo tool
    registry.register(Box::new(TodoTool::new()));

    // Register cron tool
    registry.register(Box::new(CronTool::new()));

    // Register heartbeat tool (agent self-management)
    registry.register(Box::new(HeartbeatTool::new()));

    // Register time tool
    registry.register(Box::new(TimeTool::new()));

    // Register browser tool (if browser feature enabled)
    #[cfg(feature = "browser")]
    registry.register(Box::new(BrowserTool::new()));

    // Register ACP tools for subagent spawning
    registry.register(Box::new(AcpSpawnTool::new(acp.clone(), session_store.clone())));
    registry.register(Box::new(AcpSessionTool::new(acp.clone())));

    // Register session tools
    registry.register(Box::new(SessionsListTool::new(session_store.clone())));
    registry.register(Box::new(SessionsHistoryTool::new(session_store.clone())));
    registry.register(Box::new(SessionsSendTool::new(acp.clone())));
    registry.register(Box::new(SessionsYieldTool::new(acp.clone())));
    registry.register(Box::new(SessionStatusTool::new(session_store.clone())));
    registry.register(Box::new(ApplyPatchTool::new()));

    // Register memory tool for persistent memory storage
    match MemoryTool::new().await {
        Ok(memory_tool) => {
            registry.register(Box::new(memory_tool));
            info!("MemoryTool registered successfully");
        }
        Err(e) => {
            warn!(
                "Failed to initialize MemoryTool: {}. Memory functionality will not be available.",
                e
            );
        }
    }

    // Register semantic/hybrid memory search tool
    match MemorySearchTool::new().await {
        Ok(tool) => {
            let tool = tool.with_manager_holder(memory_manager);
            registry.register(Box::new(tool));
            info!("MemorySearchTool registered successfully");
        }
        Err(e) => {
            warn!("Failed to initialize MemorySearchTool: {}. Hybrid search unavailable.", e);
        }
    }

    // Register memory get/CRUD tool
    match MemoryGetTool::new().await {
        Ok(tool) => {
            registry.register(Box::new(tool));
            info!("MemoryGetTool registered successfully");
        }
        Err(e) => {
            warn!("Failed to initialize MemoryGetTool: {}. Memory CRUD unavailable.", e);
        }
    }

    // Register MCP (Model Context Protocol) connection tool (uses shared manager)
    registry.register(Box::new(McpConnectionTool::with_manager(mcp_manager)));

    // Register plan management tool
    registry.register(Box::new(UpdatePlanTool::new()));

    // Register process management tool
    registry.register(Box::new(ProcessTool::new()));

    // Register PDF generation tool
    registry.register(Box::new(PdfTool::new()));

    // Register image tools
    registry.register(Box::new(ImageTool::new()));
    registry.register(Box::new(ImageGenerateTool::new()));

    // Register TTS tool
    registry.register(Box::new(TtsTool::new()));

    // Register STT tool
    registry.register(Box::new(SttTool::new()));

    // Register nodes/Tailscale tool
    registry.register(Box::new(NodesTool::new()));

    // Register capability discovery tool
    registry.register(Box::new(ListCapabilitiesTool::new()));

    // ── Register platform-specific capability sets ──
    {
        use crate::computer::platform::{
            CapabilityProfile, OsControlScope, PlatformCapabilityRegistry, ToolConflictStrategy,
        };

        let mut tool_reg = PlatformCapabilityRegistry::new();

        #[cfg(target_os = "linux")]
        {
            tool_reg.register(Box::new(crate::computer::platform::LinuxToolset::new()));
            tool_reg.register(Box::new(crate::computer::platform::LinuxDesktopX11Toolset::new()));
            tool_reg.register(Box::new(crate::computer::platform::LinuxDesktopWaylandToolset::new()));
        }

        #[cfg(target_os = "macos")]
        {
            tool_reg.register(Box::new(crate::computer::platform::MacosToolset::new()));
        }

        #[cfg(target_os = "windows")]
        {
            tool_reg.register(Box::new(crate::computer::platform::WindowsToolset::new()));
        }

        // Load capability profile from config
        let profile = match capabilities.profile.as_str() {
            "minimal" => CapabilityProfile::Minimal,
            "observer" => CapabilityProfile::Observer,
            "server" => CapabilityProfile::Server,
            "desktop" => CapabilityProfile::Desktop,
            "custom" => CapabilityProfile::Custom(capabilities.custom_sets.clone()),
            _ => CapabilityProfile::Full,
        };
        let max_scope = match capabilities.max_scope.as_str() {
            "read_only" => Some(OsControlScope::ReadOnly),
            "user_space" => Some(OsControlScope::UserSpace),
            "system" => Some(OsControlScope::System),
            "root" => Some(OsControlScope::Root),
            _ => None,
        };
        let disabled_sets: std::collections::HashSet<String> =
            capabilities.disabled_sets.iter().cloned().collect();

        profile.apply(&mut tool_reg);

        // Apply max_scope filter: disable sets whose scope exceeds the limit
        if let Some(limit) = max_scope {
            let to_disable: Vec<String> = tool_reg
                .all_sets()
                .iter()
                .filter(|s| s.scope() > limit)
                .map(|s| s.id().to_string())
                .collect();
            for id in to_disable {
                tool_reg.disable(&id);
            }
        }

        // Apply explicit disabled_sets filter
        for id in &disabled_sets {
            tool_reg.disable(id);
        }

        // Log detected capabilities before exporting
        let available = tool_reg.available_sets();
        if available.is_empty() {
            info!("No platform-specific tool sets detected on this host");
        } else {
            for set in &available {
                info!(
                    "Platform tool set available: {} ({}) — {}",
                    set.name(),
                    set.id(),
                    set.description()
                );
            }
        }

        tool_reg.export_to_tool_registry(&mut registry, ToolConflictStrategy::Reject);

        info!("Platform tool sets exported: {} set(s) active", available.len());
    }

    // Gate high-privilege tools behind SkillTrust::Trusted.
    // Community-trust skills see only read-only / informational tools.
    registry.mark_privileged("shell");
    registry.mark_privileged("execute_code");
    registry.mark_privileged("file_write");
    registry.mark_privileged("file_edit");
    registry.mark_privileged("delegate");
    registry.mark_privileged("acp_spawn");
    registry.mark_privileged("acp_session");
    registry.mark_privileged("memory");
    registry.mark_privileged("sessions_send");
    registry.mark_privileged("sessions_yield");
    registry.mark_privileged("subagents");
    registry.mark_privileged("apply_patch");
    registry.mark_privileged("message");
    registry.mark_privileged("process");
    registry.mark_privileged("image_generate");

    // OS control tools — privileged because they modify system state.
    registry.mark_privileged("system_inspect");
    registry.mark_privileged("service_manager");

    Ok(registry)
}

// ── Self-repair watchdog ───────────────────────────────────────────────────────

/// One watchdog cycle: find agents whose command channel is closed and respawn them.
async fn run_agent_watchdog_cycle(state: &Arc<GatewayState>) {
    const MAX_RESTARTS: u32 = 5;
    const COOLDOWN_SECS: i64 = 30;

    let dead: Vec<(String, AgentConfig)> = {
        state.agents.agents.read()
            .await
            .iter()
            .filter(|(_, h)| h.tx.is_closed())
            .map(|(id, h)| (id.clone(), h.config.clone()))
            .collect()
    };
    if dead.is_empty() {
        return;
    }

    for (agent_id, config) in dead {
        let key = format!("agent:{}", agent_id);

        let should_restart = {
            let mut records = state.agents.repair_state.records.write().await;
            let rec = records
                .entry(key.clone())
                .or_insert_with(|| RepairRecord::new(&key));
            if rec.abandoned {
                false
            } else if rec.restart_count >= MAX_RESTARTS {
                error!("Agent {} exceeded max restarts ({}), abandoning", agent_id, MAX_RESTARTS);
                rec.abandoned = true;
                false
            } else {
                !rec.last_restart_at
                    .map(|t| (chrono::Utc::now() - t).num_seconds() < COOLDOWN_SECS)
                    .unwrap_or(false)
            }
        };
        if !should_restart {
            continue;
        }

        warn!("Agent {} tx closed — attempting restart", agent_id);
        state.agents.agents.write().await.remove(&agent_id);

        match spawn_agent_inner(state.clone(), agent_id.clone(), config).await {
            Ok(_handle) => {
                let mut records = state.agents.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&agent_id));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
                info!("Agent {} restarted (attempt {})", agent_id, rec.restart_count);
                let _ = state.events.tx.send(GatewayEvent::RepairAction {
                    kind: "agent".into(),
                    target_id: agent_id,
                    description: format!(
                        "Restarted after tx closed (attempt {})",
                        rec.restart_count
                    ),
                    restart_count: rec.restart_count,
                });
            }
            Err(e) => {
                error!("Failed to restart agent {}: {}", agent_id, e);
                let mut records = state.agents.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&agent_id));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
            }
        }
    }
}

/// One watchdog cycle: check each channel's health and call `start()` if unhealthy.
async fn run_channel_watchdog_cycle(state: &Arc<GatewayState>) {
    const MAX_RESTARTS: u32 = 5;
    const COOLDOWN_SECS: i64 = 30;

    let channels: Vec<(String, Arc<dyn Channel>)> = state.channels.channels.read()
        .await
        .iter()
        .map(|(n, c)| (n.clone(), c.clone()))
        .collect();

    for (name, channel) in channels {
        let healthy = match channel.health_check().await {
            Ok(b) => b,
            Err(e) => {
                warn!("Channel {} health_check error: {}", name, e);
                false
            }
        };
        if healthy {
            continue;
        }

        let key = format!("channel:{}", name);
        let should_restart = {
            let mut records = state.agents.repair_state.records.write().await;
            let rec = records
                .entry(key.clone())
                .or_insert_with(|| RepairRecord::new(&key));
            if rec.abandoned {
                false
            } else if rec.restart_count >= MAX_RESTARTS {
                error!("Channel {} exceeded max restarts ({}), abandoning", name, MAX_RESTARTS);
                rec.abandoned = true;
                false
            } else {
                !rec.last_restart_at
                    .map(|t| (chrono::Utc::now() - t).num_seconds() < COOLDOWN_SECS)
                    .unwrap_or(false)
            }
        };
        if !should_restart {
            continue;
        }

        warn!("Channel {} unhealthy — calling start()", name);
        match channel.start().await {
            Ok(()) => {
                let mut records = state.agents.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&name));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
                info!("Channel {} restarted (attempt {})", name, rec.restart_count);
                let _ = state.events.tx.send(GatewayEvent::RepairAction {
                    kind: "channel".into(),
                    target_id: name,
                    description: format!(
                        "Restarted after health_check=false (attempt {})",
                        rec.restart_count
                    ),
                    restart_count: rec.restart_count,
                });
            }
            Err(e) => {
                error!("Failed to restart channel {}: {}", name, e);
                let mut records = state.agents.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&name));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
            }
        }
    }
}

/// Gateway-level self-repair loop — runs every 60 seconds, checks agents and channels.
async fn run_repair_loop(state: Arc<GatewayState>) {
    use std::sync::atomic::Ordering;
    state.agents.repair_state
        .loop_running
        .store(true, Ordering::Relaxed);

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        *state.agents.repair_state.last_cycle_at.write().await = Some(chrono::Utc::now());
        run_agent_watchdog_cycle(&state).await;
        run_channel_watchdog_cycle(&state).await;
    }
}

/// A snapshot of hot-reloadable configuration fields, used to compute
/// what changed between reloads.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSnapshot {
    /// Timestamp when the snapshot was taken
    pub timestamp: String,
    /// The hot-reloadable field values keyed by dotted path (e.g. "providers.openai.base_url")
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

/// Health report response structure
#[derive(Debug, Serialize)]
pub struct HealthReport {
    status: String,
    version: String,
    timestamp: String,
    overall_healthy: bool,
    subsystems: SubsystemHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    dream: Option<DreamHealthReport>,
}

/// Dream observability report embedded in the health endpoint.
#[derive(Debug, Serialize)]
pub struct DreamHealthReport {
    pub dreams_total: u64,
    pub dreams_failed: u64,
    pub memories_processed_total: u64,
    pub memories_created_total: u64,
    pub memories_removed_total: u64,
    pub memories_promoted_total: u64,
    pub memories_demoted_total: u64,
    pub dream_duration_ms_total: u64,
    pub llm_tokens_input_total: u64,
    pub llm_tokens_output_total: u64,
}

/// Per-subsystem health statuses
#[derive(Debug, Serialize)]
pub struct SubsystemHealth {
    agents: HealthStatus,
    providers: HealthStatus,
    channels: HealthStatus,
    #[serde(rename = "vector_memory")]
    vector_memory: HealthStatus,
    #[serde(rename = "memory_manager")]
    memory_manager: HealthStatus,
    cron: HealthStatus,
    plugins: HealthStatus,
    mcp: HealthStatus,
    storage: HealthStatus,
    #[serde(rename = "cost_guard")]
    cost_guard: HealthStatus,
}

/// Individual subsystem health status
#[derive(Debug, Serialize)]
pub struct HealthStatus {
    healthy: bool,
    message: String,
}

/// Simple chat handler for backwards compatibility with DaemonClient
#[derive(Debug, Deserialize)]
pub struct ChatRequestCompat {
    message: String,
    conversation_id: Option<String>,
}

/// Request body for web terminal chat
#[derive(Debug, Deserialize)]
pub struct WebTerminalChatRequest {
    /// Message content from user
    message: String,
    /// Optional conversation ID (creates new if not provided)
    conversation_id: Option<String>,
    /// Optional user ID
    user_id: Option<String>,
}

/// Response for web terminal chat
#[derive(Debug, Serialize)]
pub struct WebTerminalChatResponse {
    /// Message ID
    message_id: String,
    /// Conversation ID (new or existing)
    conversation_id: String,
    /// Status
    status: String,
}

#[derive(Debug, Deserialize)]
pub struct SetFallbackChainRequest {
    providers: Vec<String>,
}

// Vector Memory API Handlers

#[derive(Debug, Deserialize)]
pub struct MemorySearchRequest {
    query: String,
    #[serde(default = "default_memory_limit")]
    limit: usize,
    #[serde(default)]
    collection: String,
}

fn default_memory_limit() -> usize {
    10
}

#[derive(Debug, Deserialize)]
pub struct MemoryAddRequest {
    content: String,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    collection: String,
}

#[derive(Debug, Deserialize)]
pub struct RunSkillRequest {
    /// Input for the skill
    input: String,
}

/// Request body for connecting an MCP server
#[derive(Debug, Deserialize)]
pub struct McpConnectRequest {
    #[serde(default)]
    transport: String,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    url: Option<String>,
    #[serde(default = "mcp_default_timeout")]
    timeout_secs: u64,
}

/// Request body for reading a resource
#[derive(Debug, Deserialize)]
pub struct McpReadResourceRequest {
    uri: String,
}

// ── OpenAI-compatible API ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(default)]
    stream: bool,
}

/// Query parameters for model override.
#[derive(Debug, Deserialize)]
pub struct ModelOverrideQuery {
    #[serde(rename = "model")]
    model: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OpenAiChatResponse {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<OpenAiChoice>,
    usage: OpenAiUsage,
}

#[derive(Debug, Serialize)]
pub struct OpenAiChoice {
    index: u32,
    message: OpenAiResponseMessage,
    finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAiResponseMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

// ── Runtime settings CRUD ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetSettingRequest {
    key: String,
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct DenyApprovalRequest {
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddCronJobRequest {
    name: String,
    schedule: String,
    command: String,
}

// ── Mention Gate Handlers ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetMentionPolicyRequest {
    policy: crate::security::mention_gate::MentionPolicy,
}

#[derive(Debug, Deserialize)]
pub struct AddMentionPatternRequest {
    channel: String,
    pattern: String,
}

// ── Perception Summarizer Factory ──────────────────────────────────────────

/// Build the summarizer backend selected by configuration.
///
/// * `Template` — always available, zero-LLM, rule-based.
/// * `Llm` — uses the agent's LLM provider (same cost as a normal model call).
/// * `Local` — requires the `local-summarizer` feature (Qwen2.5-1.5B GGUF).
///   If the feature is missing or model loading fails, falls back to
///   `Template` with a warning so agent spawn never panics.
async fn build_summarizer(
    kind: &SummarizerKind,
    provider: Arc<dyn crate::providers::Provider>,
    model: String,
) -> Arc<dyn crate::perception::PerceptionSummarizer> {
    match kind {
        SummarizerKind::Template => {
            Arc::new(crate::perception::TemplateSummarizer::new())
        }
        SummarizerKind::Llm => Arc::new(
            crate::perception::LlmProviderSummarizer::new(provider)
                .with_model(model),
        ),
        SummarizerKind::Local => {
            #[cfg(feature = "local-summarizer")]
            {
                match crate::perception::local_summarizer::LocalLlamaSummarizer::new_auto().await
                {
                    Ok(s) => return Arc::new(s),
                    Err(e) => {
                        tracing::warn!(
                            "Local summarizer init failed: {e}; falling \
                             back to TemplateSummarizer"
                        );
                    }
                }
            }
            #[cfg(not(feature = "local-summarizer"))]
            {
                tracing::warn!(
                    "summarizer_kind = \"local\" but feature \
                     local-summarizer is not enabled; falling back to \
                     TemplateSummarizer"
                );
            }
            Arc::new(crate::perception::TemplateSummarizer::new())
        }
    }
}

#[cfg(test)]
mod api_tests;
#[cfg(test)]
pub(crate) mod state_tests;
