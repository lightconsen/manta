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
#![allow(unused_imports)]

use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{header, StatusCode},
    middleware::{from_fn, from_fn_with_state, Next},
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::{Agent, AgentConfig};
use crate::canvas::{CanvasEvent, CanvasManager};
use crate::channels::{Channel, ChannelExtension, ChannelType};
use crate::config::hot_reload::{ConfigFileType, HotReloadManager};
use crate::inbound::*;
use crate::memory::vector::{
    ApiEmbeddingProvider, CachedEmbeddingProvider, EmbeddingConfig, LocalGgufEmbeddingProvider,
    MemoryVectorStore, VectorMemoryService,
};
use crate::model_router::ModelRouter;
use crate::plugins::PluginManager;
use crate::security::pairing::DmPolicy;
use crate::tools::approval::{ApprovalDecision, ApprovalFilter, ApprovalQueue};
use crate::tools::mcp::{McpManager, McpSettings, McpToolWrapper};
use crate::tools::ToolRegistry;

pub mod auth;
pub mod commands;
pub mod hooks;
pub mod middleware;
pub mod protocol;
pub mod rate_limit;
pub mod send_policy;
pub mod webhooks;
pub mod ws;
pub mod handlers;
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
/// enable limits.  Zero means unlimited (default).
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

/// Gateway state shared across handlers
pub struct GatewayState {
    /// Configuration
    pub config: Arc<RwLock<GatewayConfig>>,
    /// Gateway startup time for uptime calculations
    pub start_time: Instant,
    /// Active channels
    pub channels: Arc<RwLock<HashMap<String, Arc<dyn Channel>>>>,
    /// Active agents by ID
    pub agents: Arc<RwLock<HashMap<String, AgentHandle>>>,
    /// Session routing table: session_id -> agent_id
    /// DEPRECATED: use `agent_router` instead for new code.
    pub session_routing: Arc<RwLock<HashMap<String, String>>>,
    /// Agent router for workspace-aware multi-agent routing.
    pub agent_router: Arc<AgentRouter>,
    /// Session to channel mapping: session_id -> (channel_name, channel_specific_id)
    /// Used to route responses back to the correct channel endpoint
    pub session_channels: Arc<RwLock<HashMap<String, (String, String)>>>,
    /// Webhook session storage: platform_key -> session_uuid
    /// Platform key format: "whatsapp:phone_number" or "feishu:user_id"
    /// Used for UUID-based session management in webhook-based channels
    pub webhook_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// Model router for multi-provider support
    pub model_router: Arc<ModelRouter>,
    /// Tool registry for all agents
    pub tool_registry: Arc<ToolRegistry>,
    /// Event broadcast channel
    pub event_tx: broadcast::Sender<GatewayEvent>,
    /// Event hook registry for intercepting/transforming events
    pub hook_registry: Arc<hooks::EventHookRegistry>,
    /// Message queue for processing
    pub message_queue: mpsc::Sender<QueuedMessage>,
    /// Canvas manager for dynamic UI
    pub canvas_manager: Arc<CanvasManager>,
    /// Plugin manager for extensibility
    pub plugin_manager: Arc<PluginManager>,
    /// ACP control plane for subagent spawning
    pub acp: Arc<AcpControlPlane>,
    /// Vector memory service for semantic search (RwLock for late initialization)
    pub vector_memory: RwLock<Option<Arc<VectorMemoryService>>>,
    /// Session search for FTS5 conversation indexing (RwLock for late initialization)
    pub session_search: RwLock<Option<Arc<crate::memory::SessionSearch>>>,
    /// Memory manager — unified orchestrator with hybrid search (Arc<RwLock> so tools
    /// and handlers can share late-initialized access without &mut GatewayState)
    pub memory_manager: Arc<RwLock<Option<Arc<crate::memory::MemoryManager>>>>,
    /// Hot reload manager for config changes (RwLock for late initialization)
    pub hot_reload: RwLock<Option<Arc<HotReloadManager>>>,
    /// Cron scheduler for scheduled jobs (RwLock for late initialization)
    pub cron_scheduler: RwLock<Option<Arc<tokio::sync::Mutex<crate::cron::cron::CronScheduler>>>>,
    /// Heartbeat wake channel sender (for requesting immediate heartbeat)
    pub heartbeat_wake_tx: RwLock<Option<tokio::sync::mpsc::Sender<crate::heartbeat::WakeRequest>>>,
    /// Heartbeat event broadcast sender (RwLock for late initialization)
    pub heartbeat_event_tx:
        RwLock<Option<tokio::sync::broadcast::Sender<crate::heartbeat::HeartbeatEvent>>>,
    /// Dream scheduler for background memory consolidation (RwLock for late initialization)
    pub dream_scheduler: RwLock<Option<crate::memory::DreamScheduler>>,
    /// Standing order manager for persistent background agent programs
    pub standing_order_manager:
        RwLock<Option<crate::standing_orders::StandingOrderManager>>,
    /// Auth manager for authentication
    pub auth_manager: Arc<crate::security::AuthManager>,
    /// DM pairing store for access control
    pub pairing_store: Arc<crate::security::pairing::PairingStore>,
    /// Device pairing store for WebSocket device auth
    pub device_pairing_store: Arc<crate::security::device_pairing::DevicePairingStore>,
    /// Command gate for slash-command permission control
    pub command_gate: Arc<crate::tools::command_gate::CommandGate>,
    /// Mention gate for controlling which mentions trigger agent responses
    pub mention_gate: Arc<crate::security::mention_gate::MentionGate>,
    /// Persistent audit log for security-relevant events (SQLite-backed)
    pub audit_log: Arc<crate::security::persistent_audit::PersistentAuditLog>,
    /// Rate limiter for API protection (legacy token bucket)
    pub rate_limiter: Arc<crate::security::RateLimiter>,
    /// Multi-tier rate limiter (sliding window per user/ip/endpoint)
    pub multi_tier_rate_limiter: Arc<crate::gateway::rate_limit::MultiTierRateLimiter>,
    /// Storage adapter for persistence
    pub storage: Arc<RwLock<dyn crate::adapters::Storage>>,
    /// Skills manager for hot-reloadable skills
    pub skills_manager: Arc<RwLock<crate::skills::SkillManager>>,
    /// Agent registry for discovered personalities (OpenClaw-style)
    pub agent_registry: Arc<RwLock<crate::agent::AgentRegistry>>,
    /// Multi-agent session manager (OpenClaw-style)
    pub session_manager: Arc<RwLock<crate::agent::SessionManager>>,
    /// SQLite-backed session store for persistent chat history
    pub session_store: Option<Arc<crate::agent::session_store::SessionStore>>,
    /// MCP manager for server connections (shared with McpConnectionTool)
    pub mcp_manager: Arc<McpManager>,
    /// Path to the config file (for runtime persistence)
    pub config_path: Option<PathBuf>,
    /// Runtime settings store — mutable key/value pairs changeable without restart.
    pub runtime_settings: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    /// Approval queue for human-in-the-loop tool policy enforcement.
    pub approval_queue: Arc<ApprovalQueue>,
    /// Self-repair loop state — tracks restart records, exposed via REST.
    pub repair_state: Arc<RepairState>,
    /// Shared live cost guard — tracks daily spend and hourly action rate
    /// across all agents. `Arc` allows every spawned agent to share one guard.
    pub cost_guard: Arc<crate::agent::CostGuard>,
    /// Reply dispatcher for routing agent responses to channels.
    pub reply_dispatcher: Arc<crate::outbound::ReplyDispatcher>,
    /// Sender for the inbound pipeline to deliver `RoutedMessage`s.
    pub routed_tx: mpsc::Sender<crate::inbound::RoutedMessage>,
    /// Inbound pipeline for processing incoming messages.
    pub inbound_pipeline: Arc<dyn crate::inbound::InboundPipeline>,
    /// Outbound pipeline for processing agent outputs.
    pub outbound_pipeline: Arc<dyn crate::outbound::OutboundPipeline>,
    /// Side effect executor for post-response actions.
    pub side_effect_executor: Arc<crate::outbound::SideEffectExecutor>,
    /// SSE streamer for real-time event streaming.
    pub sse_streamer: Arc<crate::outbound::SseStreamer>,
    /// Channel extension registry for unified channel management.
    pub channel_extensions: Arc<RwLock<crate::channels::ChannelExtensionRegistry>>,
    /// Provider SDK for dynamic provider registration and discovery.
    pub provider_sdk: Arc<RwLock<crate::providers::ProviderSdk>>,
    /// Tool SDK for dynamic tool pack registration.
    pub tool_sdk: Arc<RwLock<crate::tools::ToolSdk>>,
    /// Session message buffer for FollowUp / Collect queue modes.
    /// session_id -> buffered messages (content + metadata)
    pub session_message_buffer: Arc<RwLock<HashMap<String, Vec<BufferedMessage>>>>,
    /// OpenClaw-aligned route resolver with multi-dimensional matching.
    pub route_resolver: Arc<crate::agent::RouteResolver>,
    /// File-based transcript store for session export.
    pub transcript_store: Arc<crate::agent::TranscriptStore>,
    /// Artifact store for session-bound code snippets, documents, links.
    pub artifact_store: Arc<crate::agent::ArtifactStore>,
    /// Disk budget manager for per-session storage quota enforcement.
    pub disk_budget: Arc<crate::agent::DiskBudgetManager>,
    /// Session file manager for isolated per-session file operations.
    pub session_file_manager: Arc<crate::agent::SessionFileManager>,
    /// Group session manager for multi-member sessions with role awareness.
    pub group_session_manager: Arc<RwLock<crate::agent::GroupSessionManager>>,
    /// Browser bridge server (started when browser.bridge_enabled is true)
    #[cfg(feature = "browser")]
    pub browser_bridge: tokio::sync::RwLock<Option<crate::browser::BrowserBridge>>,
    /// Computer / desktop automation adapter (optional)
    pub computer_adapter: tokio::sync::RwLock<Option<Arc<dyn crate::computer::ComputerAdapter>>>,
    /// Log line broadcast channel for real-time log streaming to WebSocket clients
    pub log_tx: broadcast::Sender<String>,
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
                self.audit_log
                    .log(AuditEventType::AccessCheck, user_id, channel, false, &reason, None)
                    .await;
                return Err(reason);
            }

            // 2. DM Policy check
            use crate::security::pairing::DmPolicy;
            match ch_cfg.dm_policy {
                DmPolicy::Open => {}
                DmPolicy::Pairing => {
                    if !self.pairing_store.is_authorized(channel, user_id).await {
                        // Create pairing request silently and drop message
                        let _ = self
                            .pairing_store
                            .request_access(channel, user_id, None)
                            .await;
                        let reason = format!(
                            "User {} not authorized on channel {} (pairing required)",
                            user_id, channel
                        );
                        self.audit_log
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
                        && !self.pairing_store.is_authorized(channel, user_id).await
                    {
                        let reason =
                            format!("User {} not in allowlist for channel {}", user_id, channel);
                        self.audit_log
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
                self.audit_log
                    .log(AuditEventType::AccessCheck, user_id, channel, false, &reason, None)
                    .await;
                return Err(reason);
            }

            // 3b. MentionGate policy check
            if matches!(mention, crate::channels::MentionState::Mentioned) {
                let mention_allowed = self.mention_gate.check(channel, "*").await;
                if !mention_allowed {
                    let reason = format!(
                        "Mention gate blocked message on channel {} (policy: {})",
                        channel,
                        self.mention_gate.policy().await
                    );
                    self.audit_log
                        .log(AuditEventType::AccessCheck, user_id, channel, false, &reason, None)
                        .await;
                    return Err(reason);
                }
            }
        }

        // 4. Command gate check
        let decision = self.command_gate.check(user_id, content);
        if !decision.is_allowed() {
            let reason = match decision {
                crate::tools::command_gate::AccessDecision::Denied { reason, .. } => reason,
                _ => "Unknown denial reason".to_string(),
            };
            let msg = format!("Command gate denied for user {}: {}", user_id, reason);
            self.audit_log
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
        self.audit_log
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

/// Queued message for processing
#[derive(Debug)]
pub struct QueuedMessage {
    pub id: String,
    pub channel: String,
    pub user_id: String,
    pub content: String,
    pub session_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Optional model alias hint for agent routing
    pub model_alias: Option<String>,
    /// Mention state of the message (for group chat mention gating)
    pub mention: crate::channels::MentionState,
}

impl QueuedMessage {
    pub fn new(
        id: impl Into<String>,
        channel: impl Into<String>,
        user_id: impl Into<String>,
        content: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            channel: channel.into(),
            user_id: user_id.into(),
            content: content.into(),
            session_id: session_id.into(),
            timestamp: chrono::Utc::now(),
            model_alias: None,
            mention: crate::channels::MentionState::DirectMessage,
        }
    }

    pub fn with_mention(mut self, mention: crate::channels::MentionState) -> Self {
        self.mention = mention;
        self
    }

    pub fn with_model_alias(mut self, alias: impl Into<String>) -> Self {
        self.model_alias = Some(alias.into());
        self
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
    /// Create a new gateway instance
    pub async fn new(config: GatewayConfig, config_path: Option<PathBuf>) -> crate::Result<Self> {
        // Validate security configuration before proceeding
        validate_auth_config(&config)?;

        let (event_tx, _) = broadcast::channel(1000);
        let (log_tx, _) = broadcast::channel(1000);
        let (message_queue_tx, message_queue_rx) = mpsc::channel(1000);
        let (routed_tx, routed_rx) = mpsc::channel(1000);

        // Initialize storage adapter and shared SQLite pool early (needed for session_store → tool_registry)
        #[allow(clippy::type_complexity)]
        let (storage, unified_vector_store, sqlite_pool): (
            Arc<RwLock<dyn crate::adapters::Storage>>,
            Option<Arc<dyn crate::memory::VectorStore>>,
            Option<sqlx::SqlitePool>,
        ) = match config.storage.storage_type.as_str() {
            "sqlite" => {
                let db_path = config
                    .storage
                    .database_url
                    .as_ref()
                    .map(|s| std::path::PathBuf::from(s.strip_prefix("sqlite:").unwrap_or(s)))
                    .unwrap_or_else(|| crate::dirs::syscity_dir().join("data").join("syscity.db"));
                if let Some(parent) = db_path.parent() {
                    tokio::fs::create_dir_all(parent).await.ok();
                }
                if !db_path.exists() {
                    tokio::fs::File::create(&db_path).await.ok();
                }
                let db_url = format!("sqlite:///{}", db_path.display());
                info!("Connecting to SQLite storage at: {}", db_url);
                let pool = sqlx::SqlitePool::connect(&db_url).await.map_err(|e| {
                    crate::error::SyscityError::Storage {
                        context: "Failed to connect to SQLite".into(),
                        details: e.to_string(),
                    }
                })?;
                let sqlite_storage = Arc::new(crate::adapters::SqliteStorage::new(pool.clone()));
                let vector_store: Arc<dyn crate::memory::VectorStore> = sqlite_storage.clone();
                let storage: Arc<RwLock<dyn crate::adapters::Storage>> =
                    Arc::new(RwLock::new(crate::adapters::SqliteStorage::new(pool.clone())));
                (storage, Some(vector_store), Some(pool))
            }
            "file" => {
                let base_path = config.storage.base_path.as_deref().unwrap_or("./data");
                let storage = Arc::new(RwLock::new(crate::adapters::FileStorage::new(base_path)?));
                (storage, None, None)
            }
            _ => {
                let storage = Arc::new(RwLock::new(crate::adapters::InMemoryStorage::new()));
                (storage, None, None)
            }
        };

        // Create session_store early so it can be passed to tool registry
        let session_store: Option<Arc<crate::agent::session_store::SessionStore>> =
            if let Some(ref pool) = sqlite_pool {
                match crate::agent::session_store::SessionStore::from_pool(pool.clone()).await {
                    Ok(store) => {
                        info!("SessionStore initialized for persistent chat history");
                        Some(Arc::new(store))
                    }
                    Err(e) => {
                        warn!(
                            "Failed to initialize SessionStore: {}. Chat history will not persist.",
                            e
                        );
                        None
                    }
                }
            } else {
                None
            };

        // Create ACP control plane first (needed for tool registration)
        let acp_max_iter = config.acp.max_iterations;
        let acp = if let Some(ref store) = session_store {
            Arc::new(AcpControlPlane::new(acp_max_iter).with_store(store.clone()))
        } else {
            Arc::new(AcpControlPlane::new(acp_max_iter))
        };
        acp.load_persisted_sessions().await;

        // Create the shared MCP manager
        let mcp_manager = Arc::new(McpManager::new());

        // Create shared approval queue for human-in-the-loop tool policy enforcement
        let approval_queue = Arc::new(ApprovalQueue::new());

        // Shared holder for MemoryManager — populated after vector/FTS5 services start.
        // Wrapped in Arc so MemorySearchTool can observe the late-init value.
        let memory_manager_holder: Arc<
            tokio::sync::RwLock<Option<Arc<crate::memory::MemoryManager>>>,
        > = Arc::new(tokio::sync::RwLock::new(None));

        // Create tool registry with built-in tools (including ACP tools if enabled)
        let tool_registry = Arc::new(
            create_default_tool_registry(
                acp.clone(),
                mcp_manager.clone(),
                approval_queue.clone(),
                session_store.clone(),
                memory_manager_holder.clone(),
            )
            .await?,
        );

        // Initialize computer adapter.
        // Prefer remote control when configured; otherwise use local platform adapter.
        let computer_adapter: Option<Arc<dyn crate::computer::ComputerAdapter>> = if
            config.computer.enabled
        {
            if let Some(ref host) = config.computer.remote_control.host {
                let rc_config = crate::computer::RemoteControlConfig {
                    host: host.clone(),
                    user: config.computer.remote_control.user.clone().unwrap_or_else(|| {
                        std::env::var("USER").unwrap_or_else(|_| "user".to_string())
                    }),
                    port: config.computer.remote_control.port,
                    protocol: match config.computer.remote_control.protocol.as_str() {
                        "vnc" => crate::computer::RemoteProtocol::Vnc {
                            password: None,
                        },
                        "rdp" => crate::computer::RemoteProtocol::Rdp {
                            password: None,
                            domain: None,
                        },
                        _ => crate::computer::RemoteProtocol::Ssh {
                            key_path: config.computer.remote_control.key_path.clone(),
                        },
                    },
                    display: config.computer.remote_control.display.clone(),
                    ssh_extra_args: config.computer.remote_control.ssh_extra_args.clone(),
                    connect_timeout: std::time::Duration::from_secs(
                        config.computer.remote_control.timeout_secs,
                    ),
                };
                match crate::computer::RemoteControlAdapter::new(rc_config, tool_registry.clone())
                    .await
                {
                    Ok(adapter) => {
                        info!(
                            "Remote control adapter connected to {} for desktop automation",
                            host
                        );
                        Some(Arc::new(adapter))
                    }
                    Err(e) => {
                        warn!(
                            "Failed to connect remote control adapter to {}: {}. Falling back to local adapter.",
                            host, e
                        );
                        match crate::computer::create_adapter(tool_registry.clone()).await {
                            Ok(adapter) => {
                                info!("Local computer adapter initialized as fallback");
                                Some(Arc::from(adapter))
                            }
                            Err(e) => {
                                warn!("Failed to initialize local computer adapter: {}", e);
                                None
                            }
                        }
                    }
                }
            } else if crate::computer::has_display_server() {
                match crate::computer::create_adapter(tool_registry.clone()).await {
                    Ok(adapter) => {
                        info!("Computer adapter initialized for desktop automation");
                        Some(Arc::from(adapter))
                    }
                    Err(e) => {
                        warn!("Failed to initialize computer adapter: {}", e);
                        None
                    }
                }
            } else {
                warn!(
                    "No display server detected and no remote_control host configured; desktop automation disabled"
                );
                None
            }
        } else {
            None
        };

        // Initialize plugin manager
        let plugins_dir = crate::dirs::config_dir().join("plugins");
        let plugin_manager = {
            let pm = PluginManager::new(plugins_dir).await?;
            pm.set_tool_registry(tool_registry.clone()).await;
            Arc::new(pm)
        };

        // Create model router config — start empty, no hard-coded aliases.
        let mut model_router_config = crate::model_router::ModelRouterConfig::default();

        // If providers are configured (env vars or syscity.toml), create a
        // default alias from the first provider so the gateway is usable
        // immediately without requiring a UI round-trip.
        if let Some(first_provider) = config.providers.keys().next() {
            let alias = crate::model_router::ModelAlias {
                name: "default".to_string(),
                provider: first_provider.clone(),
                model: config.model.clone(),
                temperature: None,
                max_tokens: None,
            };
            model_router_config
                .aliases
                .insert("default".to_string(), alias);
            model_router_config.default_model = "default".to_string();
        }

        // Create and initialize model router early so it can be shared
        let model_router = Arc::new(crate::model_router::ModelRouter::new(model_router_config));
        for (name, provider_config) in &config.providers {
            info!("Configuring provider: {}", name);
            if let Err(e) = model_router
                .add_provider(name, provider_config.clone())
                .await
            {
                warn!("Failed to add provider '{}': {}", name, e);
            }
        }

        // Wire plugin manager to register plugin-backed providers with the model router
        {
            let mr_register = model_router.clone();
            let mr_unregister = model_router.clone();
            plugin_manager
                .set_provider_callbacks(
                    Arc::new(move |name: String, provider: Arc<dyn crate::providers::Provider + Send + Sync>| {
                        let mr = mr_register.clone();
                        tokio::spawn(async move {
                            if let Err(e) = mr.add_provider_instance(&name, provider).await {
                                warn!("Failed to register plugin provider '{}': {}", name, e);
                            }
                        });
                    }),
                    Arc::new(move |name: String| {
                        let mr = mr_unregister.clone();
                        tokio::spawn(async move {
                            if let Err(e) = mr.remove_provider(&name).await {
                                warn!("Failed to unregister plugin provider '{}': {}", name, e);
                            }
                        });
                    }),
                )
                .await;
        }

        // Create skill manager early so it can be shared with ACP builder and GatewayState
        let skills_manager =
            Arc::new(tokio::sync::RwLock::new(crate::skills::SkillManager::new().await?));

        // Configure ACP default agent builder (needs provider + tools, which are now ready)
        if let Ok(default_provider) = model_router.create_default_provider().await {
            let mut default_agent_config = config.default_agent.clone();
            default_agent_config.workspace_dir = config
                .workspace_dir
                .as_ref()
                .map(crate::dirs::resolve_tilde);
            default_agent_config.workspace_only = config.workspace_only;
            let default_tools = tool_registry.clone();
            let provider_clone = default_provider.clone();
            let model_router_clone = model_router.clone();
            let default_model = config.model.clone();
            let skills_manager_clone = Arc::clone(&skills_manager);
            acp.set_agent_builder(move || {
                crate::agent::AgentBuilder::new()
                    .config(default_agent_config.clone())
                    .provider(provider_clone.clone())
                    .tools(default_tools.clone())
                    .model_router(model_router_clone.clone())
                    .model_alias(default_model.clone())
                    .planner_model(default_model.clone())
                    .skill_manager(Arc::clone(&skills_manager_clone))
                    .build()
            })
            .await;
        } else {
            warn!("No default LLM provider available — ACP subagent spawning will fail until a provider is configured");
        }

        // Initialize security components
        let auth_manager = Arc::new(
            crate::security::AuthManager::new()
                .with_pairing_required(config.security.pairing_required),
        );
        let rate_limiter = Arc::new(crate::security::RateLimiter::new(
            config.security.rate_limit.capacity,
            config.security.rate_limit.refill_rate,
        ));

        // Initialize multi-tier rate limiter with sliding window per user/ip/endpoint
        let multi_tier_config = crate::gateway::rate_limit::MultiTierRateLimitConfig {
            global: crate::gateway::rate_limit::TierConfig {
                enabled: config.security.rate_limit.global.enabled,
                capacity: config.security.rate_limit.global.capacity,
                window_secs: config.security.rate_limit.global.window_secs,
            },
            per_user: crate::gateway::rate_limit::TierConfig {
                enabled: config.security.rate_limit.per_user.enabled,
                capacity: config.security.rate_limit.per_user.capacity,
                window_secs: config.security.rate_limit.per_user.window_secs,
            },
            per_ip: crate::gateway::rate_limit::TierConfig {
                enabled: config.security.rate_limit.per_ip.enabled,
                capacity: config.security.rate_limit.per_ip.capacity,
                window_secs: config.security.rate_limit.per_ip.window_secs,
            },
            per_endpoint: crate::gateway::rate_limit::TierConfig {
                enabled: config.security.rate_limit.per_endpoint.enabled,
                capacity: config.security.rate_limit.per_endpoint.capacity,
                window_secs: config.security.rate_limit.per_endpoint.window_secs,
            },
        };
        let multi_tier_rate_limiter =
            Arc::new(crate::gateway::rate_limit::MultiTierRateLimiter::new(multi_tier_config));

        // Create inbound / outbound pipelines (skeleton alignment)
        let agent_router = if let Some(pool) = sqlite_pool.clone() {
            let binding_store = Arc::new(
                crate::inbound::SqliteBindingStore::new(pool)
                    .await
                    .map_err(|e| crate::error::SyscityError::Storage {
                        context: "Failed to create binding store".to_string(),
                        details: e.to_string(),
                    })?,
            );
            let router = AgentRouter::new(crate::inbound::AgentRouterConfig::default())
                .with_binding_store(binding_store);
            router.load_bindings().await.ok();
            Arc::new(router)
        } else {
            Arc::new(AgentRouter::new(crate::inbound::AgentRouterConfig::default()))
        };
        let reply_dispatcher = Arc::new(crate::outbound::ReplyDispatcher::new(
            crate::outbound::ReplyDispatchConfig::default(),
        ));

        let (debounce_flush_tx, debounce_flush_rx) = mpsc::channel(256);
        let debouncer = InboundDebouncer::new(InboundDebouncerConfig::default(), debounce_flush_tx);

        let inbound_concrete = Arc::new(crate::inbound::DefaultInboundPipeline::new(
            debouncer.clone(),
            crate::inbound::MediaUnderstandingPipeline::new()
                .with_model_router(Arc::clone(&model_router)),
            crate::inbound::AutoReplyDispatch::new(
                crate::inbound::AutoReplyDispatchConfig::default(),
            ),
            crate::inbound::QueueModeResolver::new(),
            (*agent_router).clone(),
            routed_tx.clone(),
            debounce_flush_rx,
        ));
        inbound_concrete.clone().start();
        let inbound_pipeline: Arc<dyn crate::inbound::InboundPipeline> = inbound_concrete;

        let side_effect_registry = Arc::new(crate::outbound::SideEffectRegistry::new());
        let side_effect_executor =
            Arc::new(crate::outbound::SideEffectExecutor::new(side_effect_registry));
        let sse_streamer = Arc::new(crate::outbound::SseStreamer::new());
        let outbound_pipeline: Arc<dyn crate::outbound::OutboundPipeline> =
            Arc::new(crate::outbound::DefaultOutboundPipeline::new(
                reply_dispatcher.clone(),
                side_effect_executor.clone(),
                Some(sse_streamer.clone()),
                None, // trajectory_writer – optional, can be wired later
            ));

        // Create state with placeholder values for vector_memory and hot_reload
        // We'll fill them in after state creation to allow callbacks to reference state
        let state = Arc::new(GatewayState {
            config: Arc::new(RwLock::new(config.clone())),
            start_time: Instant::now(),
            config_path: config_path.clone(),
            channels: Arc::new(RwLock::new(HashMap::new())),
            agents: Arc::new(RwLock::new(HashMap::new())),
            session_routing: Arc::new(RwLock::new(HashMap::new())),
            agent_router,
            session_channels: Arc::new(RwLock::new(HashMap::new())),
            webhook_sessions: Arc::new(RwLock::new(HashMap::new())),
            model_router,
            tool_registry,
            event_tx,
            log_tx,
            hook_registry: Arc::new(hooks::EventHookRegistry::new()),
            message_queue: message_queue_tx,
            canvas_manager: Arc::new(CanvasManager::new()),
            plugin_manager,
            acp,
            vector_memory: RwLock::new(None),
            session_search: RwLock::new(None),
            memory_manager: memory_manager_holder.clone(),
            hot_reload: RwLock::new(None),
            cron_scheduler: RwLock::new(None),
            heartbeat_wake_tx: RwLock::new(None),
            heartbeat_event_tx: RwLock::new(None),
            dream_scheduler: RwLock::new(None),
            standing_order_manager: RwLock::new(None),
            auth_manager,
            pairing_store: Arc::new(crate::security::pairing::PairingStore::new()),
            device_pairing_store: Arc::new(
                crate::security::device_pairing::DevicePairingStore::new(),
            ),
            command_gate: {
                let gate = crate::tools::command_gate::CommandGate::new();
                // Web terminal and API users need User level for slash commands
                gate.set_user_level("web_user", crate::tools::command_gate::UserLevel::User);
                gate.set_user_level("api_user", crate::tools::command_gate::UserLevel::User);
                Arc::new(gate)
            },
            mention_gate: {
                let gate = crate::security::mention_gate::MentionGate::new(
                    config.security.mention_gating.policy,
                );
                for pattern in &config.security.mention_gating.allowlist {
                    gate.add_allowlist("*", pattern.clone()).await;
                }
                for pattern in &config.security.mention_gating.blocklist {
                    gate.add_blocklist("*", pattern.clone()).await;
                }
                Arc::new(gate)
            },
            audit_log: {
                let audit = if let Some(ref pool) = sqlite_pool {
                    crate::security::persistent_audit::PersistentAuditLog::with_pool(pool.clone())
                } else {
                    crate::security::persistent_audit::PersistentAuditLog::new()
                };
                Arc::new(audit)
            },
            rate_limiter,
            multi_tier_rate_limiter,
            storage,
            skills_manager,
            agent_registry: Arc::new(RwLock::new(crate::agent::AgentRegistry::new())),
            session_manager: Arc::new(RwLock::new(crate::agent::SessionManager::new())),
            session_store,
            mcp_manager: mcp_manager.clone(),
            runtime_settings: Arc::new(RwLock::new(HashMap::new())),
            approval_queue,
            repair_state: Arc::new(RepairState::new()),
            cost_guard: crate::agent::CostGuard::new(
                config.cost_guard.daily_limit_cents,
                config.cost_guard.hourly_action_limit,
            ),
            reply_dispatcher,
            routed_tx,
            inbound_pipeline,
            outbound_pipeline,
            side_effect_executor: side_effect_executor.clone(),
            sse_streamer: sse_streamer.clone(),
            channel_extensions: Arc::new(RwLock::new(
                crate::channels::ChannelExtensionRegistry::new(),
            )),
            provider_sdk: Arc::new(RwLock::new(crate::providers::ProviderSdk::new())),
            tool_sdk: Arc::new(RwLock::new(crate::tools::ToolSdk::new())),
            session_message_buffer: Arc::new(RwLock::new(HashMap::new())),
            route_resolver: Arc::new(crate::agent::RouteResolver::new("default")),
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
            group_session_manager: Arc::new(RwLock::new(crate::agent::GroupSessionManager::new())),
            #[cfg(feature = "browser")]
            browser_bridge: tokio::sync::RwLock::new(None),
            computer_adapter: tokio::sync::RwLock::new(computer_adapter),
        });

        // Attach SessionStore to SessionManager for unified session model
        if let Some(ref store) = state.session_store {
            let mut mgr = state.session_manager.write().await;
            mgr.with_store(store.clone());
        }

        // Initialize audit table (SQLite-backed persistent audit log)
        if let Err(e) = state.audit_log.init().await {
            warn!("Failed to initialize persistent audit log: {}", e);
        }

        // Dynamically register OpenClaw-compatible tools that need GatewayState
        state
            .tool_registry
            .register_dynamic(Arc::new(crate::tools::AgentsListTool::new(
                state.agent_registry.clone(),
            )));
        state
            .tool_registry
            .register_dynamic(Arc::new(crate::tools::GatewayTool::new(state.clone())));
        state
            .tool_registry
            .register_dynamic(Arc::new(crate::tools::MessageTool::new(state.clone())));
        state
            .tool_registry
            .register_dynamic(Arc::new(crate::tools::CanvasTool::new(
                state.canvas_manager.clone(),
            )));

        // Sync ProviderSdk / ToolSdk with existing registries (skeleton alignment)
        {
            let mut provider_sdk = state.provider_sdk.write().await;
            provider_sdk
                .sync_from_model_router(&state.model_router)
                .await;
        }
        {
            let mut tool_sdk = state.tool_sdk.write().await;
            tool_sdk.sync_from_tool_registry(&state.tool_registry);
        }

        // Initialize vector memory service if enabled
        if config.vector_memory.enabled {
            info!("Initializing vector memory service...");

            let embedding_provider: Option<Arc<dyn crate::memory::vector::EmbeddingProvider>> =
                match config.vector_memory.provider {
                    EmbeddingProviderType::OpenAi => {
                        if let Some(ref api_key) = config.vector_memory.embedding_api_key {
                            info!("Using OpenAI embedding provider");
                            let mut provider = ApiEmbeddingProvider::new(
                                api_key.clone(),
                                config.vector_memory.embedding_model.clone(),
                                config.vector_memory.embedding_dimension,
                            );
                            if let Some(ref base_url) = config.vector_memory.api_base_url {
                                provider = provider.with_base_url(base_url.clone());
                            }
                            Some(Arc::new(provider))
                        } else {
                            warn!("OpenAI embedding provider requires an API key");
                            None
                        }
                    }
                    EmbeddingProviderType::LocalGguf => {
                        #[cfg(feature = "local-embeddings")]
                        {
                            if let Some(ref model_path) = config.vector_memory.local_model_path {
                                info!("Using local GGUF embedding provider");
                                use crate::memory::local_embeddings::ModelSource;
                                let source = ModelSource::parse(model_path);
                                let provider = LocalGgufEmbeddingProvider::create(
                                    source,
                                    config.vector_memory.embedding_dimension,
                                )
                                .await;
                                if provider.is_fts_only() {
                                    if let Some(reason) = provider.fts_reason() {
                                        warn!("Local GGUF provider in FTS-only mode: {}", reason);
                                    } else {
                                        info!("Local GGUF provider initialized, will load model on first use");
                                    }
                                } else {
                                    info!("GGUF model configured from {}", model_path);
                                }
                                Some(Arc::new(provider))
                            } else {
                                warn!(
                                    "Local GGUF provider requires 'local_model_path' configuration"
                                );
                                None
                            }
                        }
                        #[cfg(not(feature = "local-embeddings"))]
                        {
                            warn!("Local GGUF provider requires 'local-embeddings' feature. Build with: cargo build --features local-embeddings");
                            None
                        }
                    }
                };

            if let Some(embedding_provider) = embedding_provider {
                // Use unified storage as the vector store (if it's SqliteStorage)
                // For non-SQLite storage, fall back to in-memory vector store
                let vector_store: Arc<dyn crate::memory::VectorStore> = match unified_vector_store {
                    Some(store) => {
                        info!("Using unified SQLite storage for vector store");
                        store
                    }
                    None => {
                        info!("Using in-memory vector store (unified storage requires 'sqlite' storage type)");
                        Arc::new(MemoryVectorStore::new(config.vector_memory.embedding_dimension))
                    }
                };

                // Create embedding config for the service
                let embedding_config = EmbeddingConfig {
                    model: config.vector_memory.embedding_model.clone(),
                    chunk_size: 512,
                    chunk_overlap: 50,
                    batch_size: 32,
                };

                // Wrap with a SHA-256 dedup cache (1 024-entry FIFO) to avoid
                // re-embedding identical text across requests.
                let cached_provider = CachedEmbeddingProvider::new(embedding_provider, 1024);
                let service = Arc::new(VectorMemoryService::new(
                    Arc::new(cached_provider),
                    vector_store,
                    &embedding_config,
                ));
                info!(
                    "✅ Vector memory service initialized with {:?} provider",
                    config.vector_memory.provider
                );
                *state.vector_memory.write().await = Some(service);
            } else {
                warn!("Vector memory enabled but no suitable provider available");
            }
        } else {
            info!("Vector memory service disabled");
        }

        // Initialize SessionSearch (FTS5) and MemoryManager (hybrid search) if we have SQLite
        if let Some(pool) = sqlite_pool {
            info!("Initializing session search (FTS5)...");
            let session_search = Arc::new(crate::memory::SessionSearch::new(pool.clone()));
            if let Err(e) = session_search.initialize().await {
                warn!("Failed to initialize session search: {}", e);
            } else {
                info!("✅ Session search (FTS5) initialized");
                *state.session_search.write().await = Some(session_search.clone());

                // Create MemoryManager with hybrid search enabled if we also have vector_memory
                let vector_guard = state.vector_memory.read().await;
                if let Some(ref vector_svc) = *vector_guard {
                    info!("Initializing MemoryManager with hybrid search...");
                    // Create store from the existing pool (shared connection)
                    let store = Arc::new(
                        crate::memory::UnifiedStore::new_with_pool(pool.clone())
                            .await
                            .map_err(|e| crate::error::SyscityError::Storage {
                                context: "Failed to create UnifiedStore".into(),
                                details: e.to_string(),
                            })?,
                    );
                    let mm = crate::memory::MemoryManager::new(
                        store.clone(),
                        store,
                        crate::memory::MemoryManagerConfig::default(),
                    )
                    .with_vector_service(vector_svc.clone())
                    .with_session_search(session_search);
                    *state.memory_manager.write().await = Some(Arc::new(mm));
                    info!("✅ MemoryManager with hybrid search initialized");
                } else {
                    info!("Initializing MemoryManager (vector search disabled)...");
                    let store = Arc::new(
                        crate::memory::UnifiedStore::new_with_pool(pool.clone())
                            .await
                            .map_err(|e| crate::error::SyscityError::Storage {
                                context: "Failed to create UnifiedStore".into(),
                                details: e.to_string(),
                            })?,
                    );
                    let mm = crate::memory::MemoryManager::new(
                        store.clone(),
                        store,
                        crate::memory::MemoryManagerConfig::default(),
                    )
                    .with_session_search(session_search);
                    *state.memory_manager.write().await = Some(Arc::new(mm));
                    info!("✅ MemoryManager initialized (without vector search)");
                }
            }
        } else {
            info!("SQLite not in use; session search and hybrid memory disabled");
        }

        // Initialize hot reload manager if enabled
        if config.hot_reload.enabled {
            info!("Initializing hot reload manager...");
            match HotReloadManager::new() {
                Ok(manager) => {
                    let manager = Arc::new(manager);
                    info!("Hot reload manager initialized");
                    *state.hot_reload.write().await = Some(manager);
                }
                Err(e) => {
                    warn!("Failed to initialize hot reload manager: {}", e);
                }
            }
        } else {
            info!("Hot reload disabled");
        }

        // Initialize cron scheduler if enabled
        if config.cron.enabled {
            info!("Initializing advanced cron scheduler...");
            use crate::cron::cron::{AnnounceDelivery, CronScheduler};
            let (cron_scheduler, command_rx) = CronScheduler::new();
            let cron_scheduler =
                cron_scheduler.with_store_path(crate::dirs::cron_dir().join("jobs.json"));
            let cron_scheduler = Arc::new(tokio::sync::Mutex::new(cron_scheduler));

            // Wire up announce delivery → SSE broadcast
            let (announce_tx, mut announce_rx) = mpsc::channel::<AnnounceDelivery>(64);
            {
                let mut scheduler = cron_scheduler.lock().await;
                scheduler.set_announce_tx(announce_tx);
            }
            let event_tx_announce = state.event_tx.clone();
            tokio::spawn(async move {
                while let Some(delivery) = announce_rx.recv().await {
                    info!("Cron announce → {}:{}", delivery.channel, delivery.to);
                    match event_tx_announce.send(GatewayEvent::CronAnnounce {
                        channel: delivery.channel,
                        to: delivery.to,
                        message: delivery.message.clone(),
                    }) {
                        Ok(receiver_count) => {
                            info!("Cron announce broadcast to {} receivers", receiver_count)
                        }
                        Err(e) => warn!("Failed to broadcast cron announce: {}", e),
                    }
                }
            });

            // Start the scheduler in a background task
            let cron_scheduler_clone = Arc::clone(&cron_scheduler);
            tokio::spawn(async move {
                let mut scheduler = cron_scheduler_clone.lock().await;
                if let Err(e) = scheduler.start(command_rx).await {
                    warn!("Advanced cron scheduler failed: {}", e);
                }
            });
            *state.cron_scheduler.write().await = Some(cron_scheduler.clone());
            info!("✅ Advanced cron scheduler initialized");

            // Wire the scheduler into CronTool so it can delegate operations
            crate::tools::CronTool::set_scheduler(cron_scheduler);
        } else {
            info!("Cron scheduler disabled");
        }

        // Wire side-effect executor with runtime context (memory + cron)
        let side_effect_ctx = crate::outbound::SideEffectContext {
            memory_manager: state.memory_manager.read().await.clone(),
            cron_scheduler: state.cron_scheduler.read().await.clone(),
            webhook_client: Some(Arc::new(
                reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                    .unwrap_or_default(),
            )),
        };
        state
            .side_effect_executor
            .set_context(side_effect_ctx)
            .await;
        info!("✅ SideEffectExecutor context wired");

        // Start message processing worker (legacy QueuedMessage path)
        tokio::spawn(Self::process_message_queue(state.clone(), message_queue_rx));
        // Start routed-message worker (new InboundPipeline path)
        tokio::spawn(Self::process_routed_messages(state.clone(), routed_rx));

        Ok(Self { state, config })
    }

    /// Return a clone of the internal `ModelRouter` arc.
    ///
    /// Primarily used in integration / E2E tests to inject a mock provider
    /// before calling `start()`.
    pub fn model_router(&self) -> Arc<crate::model_router::ModelRouter> {
        self.state.model_router.clone()
    }

    /// Return a clone of the internal `ToolRegistry` arc.
    pub fn tool_registry(&self) -> Arc<crate::tools::ToolRegistry> {
        self.state.tool_registry.clone()
    }

    /// Start the gateway
    pub async fn start(&self) -> crate::Result<()> {
        info!("Starting Syscity Gateway control plane...");

        // Initialize plugins if enabled
        if self.config.plugins.enabled {
            if self.config.plugins.auto_load {
                if let Err(e) = self.state.plugin_manager.initialize().await {
                    warn!("Failed to initialize plugins: {}", e);
                }

                // Watch WASM files for hot-reload
                if let Some(ref hot_reload) = *self.state.hot_reload.read().await {
                    let plugins = self.state.plugin_manager.list_plugins().await;
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
            let mut skills_manager = self.state.skills_manager.write().await;
            match skills_manager.initialize().await {
                Ok(count) => info!("✅ Skills manager initialized with {} skills", count),
                Err(e) => warn!("Failed to initialize skills manager: {}", e),
            }
        }

        // Initialize hot reload if enabled
        let hot_reload = self.state.hot_reload.read().await.clone();
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
            tokio::spawn(async move {
                if let Err(e) = hot_reload_clone.run().await {
                    error!("Hot reload error: {}", e);
                }
            });

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

        // Discover agents from agents/ directory (OpenClaw-style auto-discovery)
        {
            let mut registry = self.state.agent_registry.write().await;
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

        // Auto-connect MCP servers (9.1, 9.2)
        self.init_mcp_servers().await;

        // Initialize configured channels
        self.init_channels().await?;

        // Start dream scheduler if enabled
        if self.config.dreaming.enabled {
            if let Some(ref mm) = *self.state.memory_manager.read().await {
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
                        crate::memory::DreamEngine::new(dream_config, tier_system_config);
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
                    self.state.dream_scheduler.write().await.replace(scheduler);
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
            self.state
                .standing_order_manager
                .write()
                .await
                .replace(manager);
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
                        let mut bridge_lock = self.state.browser_bridge.write().await;
                        *bridge_lock = Some(bridge);
                    }
                    let mut settings = self.state.runtime_settings.write().await;
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
            let mut approval_rx = self.state.approval_queue.event_tx.subscribe();
            let event_tx = self.state.event_tx.clone();
            tokio::spawn(async move {
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
        }

        // Start gateway-level self-repair watchdog (60 s interval)
        tokio::spawn(run_repair_loop(self.state.clone()));

        // Start heartbeat runner if enabled
        if self.config.heartbeat.enabled {
            let runner = crate::heartbeat::HeartbeatRunner::new(self.state.clone());
            let wake_tx = runner.wake_sender();
            let event_tx = runner.event_tx.clone();
            *self.state.heartbeat_wake_tx.write().await = Some(wake_tx.clone());
            *self.state.heartbeat_event_tx.write().await = Some(event_tx);
            tokio::spawn(async move {
                runner.start().await;
            });
            info!("Heartbeat runner started");

            // Wire heartbeat wake sender into cron scheduler so cron jobs
            // with wake_mode: heartbeat_nuke can trigger immediate heartbeats
            if let Some(ref cron_arc) = *self.state.cron_scheduler.read().await {
                let mut scheduler = cron_arc.lock().await;
                scheduler.set_heartbeat_wake_tx(wake_tx);
                info!("Cron heartbeat wake integration enabled");
            }
        }

        // Start log tail broadcaster for real-time log streaming
        {
            let log_tx = self.state.log_tx.clone();
            tokio::spawn(async move {
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
            info!("Log tail broadcaster started");
        }

        // Run the server
        axum::serve(listener, app).await.map_err(|e| {
            crate::error::SyscityError::ExternalService {
                source: "Gateway server error".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

        // Stop dream scheduler on shutdown
        if let Some(mut scheduler) = self.state.dream_scheduler.write().await.take() {
            scheduler.stop().await;
            info!("Dream scheduler stopped");
        }

        // Stop standing orders manager on shutdown
        if let Some(mut manager) = self.state.standing_order_manager.write().await.take() {
            manager.stop().await;
            info!("Standing orders manager stopped");
        }

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
            // WebSocket canvas
            .route("/ws/canvas/:id", get(canvas_ws_handler))
            // Syscity as MCP server -- Streamable-HTTP endpoint
            .route("/mcp", post(syscity_as_mcp_server_handler))
            // Admin redirect -- management UI moved to CLI
            .route("/admin", get(admin_redirect_handler))
            // Computer / desktop automation API
            .route("/api/v1/computer/screenshot", get(computer_screenshot_handler))
            .route("/api/v1/computer/execute", post(computer_execute_handler))
            .route("/api/v1/computer/status", get(computer_status_handler))
            .layer(from_fn_with_state(state.clone(), middleware::auth_middleware));

        let essential_router = essential_public_router.merge(essential_auth_router);

        // Apply remaining middleware layers to essential routes
        // (order matters - applied in reverse)
        let admin_router = essential_router
            .layer(from_fn_with_state(state.clone(), middleware::rate_limit_middleware))
            .layer(from_fn_with_state(state.clone(), auth::session_cookie_middleware))
            .layer(from_fn(middleware::tailscale_only_middleware))
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
        spawn_agent_inner(self.state.clone(), id, config).await
    }
}

/// Free function that spawns an agent — callable from both `Gateway::spawn_agent`
/// and the self-repair watchdog loop.
async fn spawn_agent_inner(
    state: Arc<GatewayState>,
    id: String,
    mut config: AgentConfig,
) -> crate::Result<()> {
    config.agent_id = Some(id.clone());
    info!("Spawning agent: {}", id);

    let (tx, mut rx) = mpsc::channel(100);

    // Create provider from model router
    let provider: Arc<dyn crate::providers::Provider> =
        state.model_router.create_default_provider().await?;

    // Get tool registry from state
    let tools = state.tool_registry.clone();

    // Get the model from config for this agent
    let model = state.config.read().await.model.clone();

    // Create the actual Agent instance with model, memory manager, chat history,
    // shared cost guard, and session management stores.
    let memory_manager = state.memory_manager.read().await.clone();
    let cost_guard = Arc::clone(&state.cost_guard);

    // Read computer config for the agent
    let computer_config = {
        let cfg = state.config.read().await;
        crate::computer::LoopConfig {
            max_steps: cfg.computer.max_steps,
            settle_delay_ms: cfg.computer.settle_delay_ms,
            ..Default::default()
        }
    };
    let computer_adapter = state.computer_adapter.read().await.clone();

    let agent = if let Some(ref mm) = memory_manager {
        let chat_history = mm.chat_history();
        let mut builder = Agent::new(config.clone(), provider, tools)
            .with_model(model.clone())
            .with_memory_manager(mm.clone())
            .with_chat_history(chat_history)
            .with_cost_guard(cost_guard)
            .with_transcript_store(Arc::clone(&state.transcript_store))
            .with_artifact_store(Arc::clone(&state.artifact_store))
            .with_disk_budget(Arc::clone(&state.disk_budget))
            .with_session_file_manager(Arc::clone(&state.session_file_manager))
            .with_model_router(Arc::clone(&state.model_router))
            .with_skill_manager(Arc::clone(&state.skills_manager))
            .with_model_alias(model.clone());
        if let Some(adapter) = computer_adapter.clone() {
            builder = builder
                .with_computer_adapter(adapter)
                .with_computer_config(computer_config);
        }
        Arc::new(builder)
    } else {
        let mut builder = Agent::new(config.clone(), provider, tools)
            .with_model(model.clone())
            .with_cost_guard(cost_guard)
            .with_skill_manager(Arc::clone(&state.skills_manager))
            .with_transcript_store(Arc::clone(&state.transcript_store))
            .with_artifact_store(Arc::clone(&state.artifact_store))
            .with_disk_budget(Arc::clone(&state.disk_budget))
            .with_session_file_manager(Arc::clone(&state.session_file_manager))
            .with_model_router(Arc::clone(&state.model_router))
            .with_model_alias(model.clone());
        if let Some(adapter) = computer_adapter.clone() {
            builder = builder
                .with_computer_adapter(adapter)
                .with_computer_config(computer_config);
        }
        Arc::new(builder)
    };

    // Wire the new agent into the cron scheduler so routine (agent-target)
    // jobs can run.  Only the first agent is wired; subsequent agents keep
    // the first one active unless explicitly overwritten.
    {
        let cron_guard = state.cron_scheduler.read().await;
        if let Some(ref cron_arc) = *cron_guard {
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
        let mut agents = state.agents.write().await;
        agents.insert(id.clone(), handle);
    }

    // Start agent processing loop
    let agent_id = id.clone();

    tokio::spawn(async move {
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
                        let _ = state.event_tx.send(GatewayEvent::AgentStatus {
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
                                                state.event_tx.send(GatewayEvent::ToolCalling {
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
                                            let _ = state.event_tx.send(GatewayEvent::ToolResult {
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
                                            let _ = state.event_tx.send(GatewayEvent::Completed {
                                                session_id: session_id.clone(),
                                                agent_id: agent_id.clone(),
                                                response,
                                            });
                                        }
                                        crate::agent::ProgressEvent::Error { message } => {
                                            let _ = state.event_tx.send(GatewayEvent::ProcessingError {
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
                            let sessions = state.session_channels.read().await;
                            sessions
                                .get(&session_id)
                                .map(|(_, cid)| cid.clone())
                                .unwrap_or_else(|| session_id.clone())
                        };

                        // Generate run_id for this agent execution (OpenClaw-style run tracking)
                        let run_id = uuid::Uuid::new_v4().to_string();

                        // Persist assistant response to session history
                        if let Some(ref store) = state.session_store {
                            if let Err(e) = store
                                .append_message(
                                    &session_id,
                                    "assistant",
                                    &response_content,
                                    None,
                                    None,
                                    None,
                                    Some(&session_id), // transcript_id defaults to session_id
                                    Some(&run_id),
                                )
                                .await
                            {
                                warn!("Failed to save assistant message to session history: {}", e);
                            }
                        }

                        // Send response event
                        info!("DEBUG: Agent {} sending AgentResponse for session {} (conversation: {})", agent_id, session_id, conversation_id);
                        let _ = state.event_tx.send(GatewayEvent::AgentResponse {
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
                        let outbound_result = state.outbound_pipeline.process(outbound_ctx).await;

                        // Apply canvas updates if the pipeline produced any
                        if let Some(canvas_update) = outbound_result.canvas_update {
                            state.canvas_manager.apply_update(&session_id, canvas_update).await;
                        }

                        // Update status to idle
                        let _ = state.event_tx.send(GatewayEvent::AgentStatus {
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
                            let mut agents = state.agents.write().await;
                            if let Some(handle) = agents.get_mut(&agent_id) {
                                handle.config = new_config.clone();
                                info!("Agent {} configuration updated", agent_id);
                            }
                        }
                        // Send status update
                        let _ = state.event_tx.send(GatewayEvent::AgentStatus {
                            agent_id: agent_id.clone(),
                            status: AgentStatus::Idle,
                        });
                    }
                    AgentCommand::Shutdown => {
                        info!("Agent {} shutting down", agent_id);
                        let _ = state.event_tx.send(GatewayEvent::AgentStatus {
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

    Ok(())
}

impl Gateway {
    /// Spawn an agent from its personality (on-demand spawning)
    /// Returns true if agent was spawned, false if already exists
    pub async fn spawn_agent_from_personality(&self, agent_id: &str) -> crate::Result<bool> {
        // Check if agent already exists
        {
            let agents = self.state.agents.read().await;
            if agents.contains_key(agent_id) {
                return Ok(false);
            }
        }

        // Get personality from registry
        let personality = {
            let registry = self.state.agent_registry.read().await;
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
            let registry = self.state.agent_registry.read().await;
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
        let mut registry = self.state.channel_extensions.write().await;
        registry.register(ext.clone());
        info!("Registered channel extension: {}", ext.name());
    }

    /// Get or spawn agent by ID (on-demand)
    pub async fn get_or_spawn_agent(&self, agent_id: &str) -> crate::Result<Option<AgentHandle>> {
        // First check if already spawned
        {
            let agents = self.state.agents.read().await;
            if let Some(handle) = agents.get(agent_id) {
                return Ok(Some(handle.clone()));
            }
        }

        // Try to spawn from personality
        match self.spawn_agent_from_personality(agent_id).await {
            Ok(true) | Ok(false) => {
                // Now get the spawned agent
                let agents = self.state.agents.read().await;
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
                .state
                .mcp_manager
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

                    if let Some(client_arc) = self.state.mcp_manager.get_client(server_id).await {
                        for tool in tools.iter().take(max_tools) {
                            let wrapper =
                                Arc::new(McpToolWrapper::new(client_arc.clone(), server_id, tool));
                            self.state.tool_registry.register_dynamic(wrapper);
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
            if self.state.channels.read().await.contains_key(name) {
                info!("Channel {} already running, skipping", name);
                continue;
            }

            self.init_single_channel(name, config).await?;
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
            self.state
                .agent_router
                .set_channel_default(name, agent_name.to_string(), None)
                .await;

            // Create the channel extension
            let ext = Arc::new(crate::channels::TelegramChannelExtension::new(
                channel.clone(),
                self.state.session_channels.clone(),
            ));

            // Create inbound channel: extension -> inbound pipeline
            let (inbound_tx, mut inbound_rx) =
                mpsc::channel::<crate::channels::IncomingMessage>(1000);

            // Spawn extension inbound task (Telegram bot -> inbound pipeline)
            let ext_inbound = ext.clone();
            tokio::spawn(async move {
                if let Err(e) = ext_inbound.run_inbound(inbound_tx).await {
                    error!("Telegram extension inbound task failed: {}", e);
                }
            });

            // Bridge inbound messages into the pipeline
            let state_clone = self.state.clone();
            tokio::spawn(async move {
                while let Some(message) = inbound_rx.recv().await {
                    if let Some(routed) = state_clone.inbound_pipeline.process(message).await {
                        info!(
                            "Telegram message routed through pipeline: agent={}",
                            routed.agent_id
                        );
                    } else {
                        info!("Telegram message absorbed by pipeline (debounced or suppressed)");
                    }
                }
            });

            // Create outbound channel: reply dispatcher -> extension outbound
            let (outbound_tx, outbound_rx) =
                mpsc::channel::<crate::channels::OutgoingMessage>(1000);

            // Spawn extension outbound task (outbound pipeline -> Telegram)
            let ext_outbound = ext.clone();
            tokio::spawn(async move {
                if let Err(e) = ext_outbound.run_outbound(outbound_rx).await {
                    error!("Telegram extension outbound task failed: {}", e);
                }
            });

            // Register a bridge with the reply dispatcher so outbound pipeline
            // messages flow into the extension's run_outbound.
            let bridge = Arc::new(crate::channels::ChannelSenderBridge::new(name, outbound_tx));
            self.state
                .reply_dispatcher
                .register_channel(name, bridge)
                .await;

            // Register extension in the extension registry
            self.register_channel_extension(ext).await;

            // Keep the raw channel in the channels map for direct access
            self.state
                .channels
                .write()
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

            // Bridge inbound messages into the pipeline
            let state_clone = self.state.clone();
            tokio::spawn(async move {
                while let Some(msg) = inbound_rx.recv().await {
                    state_clone.inbound_pipeline.process(msg).await;
                }
            });

            let channel_name = name.to_string();
            let channel_for_task = channel.clone();
            tokio::spawn(async move {
                if let Err(e) = channel_for_task.start().await {
                    error!("Discord channel {} failed: {}", channel_name, e);
                }
            });
            self.state
                .reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state
                .channels
                .write()
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

            // Bridge inbound messages into the pipeline
            let state_clone = self.state.clone();
            tokio::spawn(async move {
                while let Some(msg) = inbound_rx.recv().await {
                    state_clone.inbound_pipeline.process(msg).await;
                }
            });

            let channel_name = name.to_string();
            let channel_for_task = channel.clone();
            tokio::spawn(async move {
                if let Err(e) = channel_for_task.start().await {
                    error!("Slack channel {} failed: {}", channel_name, e);
                }
            });
            self.state
                .reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state
                .channels
                .write()
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
            self.state
                .reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state
                .channels
                .write()
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
        if let (Some(app_id), Some(app_secret)) = (
            config.credentials.get("app_id"),
            config.credentials.get("app_secret"),
        ) {
            let lark_config =
                crate::channels::lark::LarkConfig::new(app_id, app_secret);

            let channel = Arc::new(crate::channels::lark::LarkChannel::new(lark_config));
            let channel_name = name.to_string();
            let channel_for_task = channel.clone();
            tokio::spawn(async move {
                if let Err(e) = channel_for_task.start().await {
                    error!("Feishu channel {} failed: {}", channel_name, e);
                }
            });
            self.state
                .reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state
                .channels
                .write()
                .await
                .insert(name.to_string(), channel);
            info!("✅ Feishu channel '{}' initialized (inbound via webhook)", name);
        } else {
            warn!(
                "Feishu channel '{}' missing 'app_id' or 'app_secret' in credentials",
                name
            );
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

            // Bridge inbound messages into the pipeline
            let state_clone = self.state.clone();
            tokio::spawn(async move {
                while let Some(msg) = inbound_rx.recv().await {
                    state_clone.inbound_pipeline.process(msg).await;
                }
            });

            let channel_name = name.to_string();
            let channel_for_task = channel.clone();
            tokio::spawn(async move {
                if let Err(e) = channel_for_task.start().await {
                    error!("QQ channel {} failed: {}", channel_name, e);
                }
            });
            self.state
                .reply_dispatcher
                .register_channel(name, channel.clone())
                .await;
            self.state
                .channels
                .write()
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

    /// Process message queue
    async fn process_message_queue(
        state: Arc<GatewayState>,
        mut rx: mpsc::Receiver<QueuedMessage>,
    ) {
        while let Some(msg) = rx.recv().await {
            info!("Processing queued message: {}", msg.id);

            // Centralized access check (blocklist, DM policy, mention, command gate)
            if let Err(reason) = state
                .check_incoming_access(&msg.channel, &msg.user_id, &msg.content, &msg.mention)
                .await
            {
                debug!("Message dropped: {}", reason);
                continue;
            }

            // Convert QueuedMessage to IncomingMessage and route through
            // the inbound pipeline (debounce -> media -> dispatch -> queue -> router).
            let incoming =
                crate::channels::IncomingMessage::new(msg.user_id, msg.session_id, msg.content)
                    .with_provenance(crate::channels::InputProvenance::ExternalUser {
                        channel: msg.channel,
                        is_direct: true,
                    });

            let _ = state.inbound_pipeline.process(incoming).await;
            // The pipeline forwards RoutedMessage to routed_tx; process_routed_messages
            // handles the actual agent dispatch.
        }
    }

    /// Process routed messages from the inbound pipeline.
    ///
    /// Converts `RoutedMessage` into `AgentCommand::ProcessMessage` and
    /// forwards it to the resolved agent, respecting `QueueMode`.
    async fn process_routed_messages(
        state: Arc<GatewayState>,
        mut rx: mpsc::Receiver<crate::inbound::RoutedMessage>,
    ) {
        while let Some(routed) = rx.recv().await {
            if routed.suppress_delivery {
                debug!("Suppressing delivery for session {}", routed.incoming.conversation_id.0);
                continue;
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
                let groups = state.group_session_manager.read().await;
                if let Some(group) = groups.get_group(&session_id) {
                    let group = group.read().await;
                    if !group.is_member(user_id) {
                        warn!(
                            "User {} is not a member of group session {}, dropping message",
                            user_id, session_id
                        );
                        continue;
                    }
                    if let Some(member) = group.get_member(user_id) {
                        if !member.role.can_participate() {
                            warn!(
                                "User {} (role: {}) cannot participate in group session {}, dropping message",
                                user_id, member.role, session_id
                            );
                            continue;
                        }
                    }
                }
            }

            match routed.queue_mode {
                crate::inbound::QueueMode::Interrupt => {
                    // Clear any buffered messages for this session
                    {
                        let mut buffers = state.session_message_buffer.write().await;
                        buffers.remove(&session_id);
                    }
                    Self::send_to_agent(
                        &state,
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
                        let agents = state.agents.read().await;
                        if let Some(agent) = agents.get(&agent_id) {
                            let _ = agent.tx.send(AgentCommand::Cancel).await;
                        }
                    }
                    // Small delay to let cancel take effect
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    Self::send_to_agent(
                        &state,
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
                        let mut buffers = state.session_message_buffer.write().await;
                        let buffer = buffers.entry(session_id.clone()).or_default();
                        buffer.push(BufferedMessage {
                            content: routed.incoming.content.clone(),
                            user_id: routed.incoming.user_id.0.clone(),
                            channel: channel.clone(),
                        });
                        buffer.len() >= 5 // Max 5 messages before forced flush
                    };

                    if should_flush {
                        Self::flush_session_buffer(&state, &agent_id, &session_id).await;
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
                        let buffers = state.session_message_buffer.read().await;
                        buffers
                            .get(&session_id)
                            .map(|b| !b.is_empty())
                            .unwrap_or(false)
                    };

                    if has_buffered {
                        Self::flush_session_buffer(&state, &agent_id, &session_id).await;
                    } else {
                        // No buffer to flush; treat as normal message
                        Self::send_to_agent(
                            &state,
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
                        &state,
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
    }

    /// Flush buffered messages for a session and send as a single batch.
    async fn flush_session_buffer(state: &Arc<GatewayState>, agent_id: &str, session_id: &str) {
        let messages: Vec<BufferedMessage> = {
            let mut buffers = state.session_message_buffer.write().await;
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
        let agents = state.agents.read().await;
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
            let s = state.runtime_settings.read().await;
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
            let s = state.runtime_settings.read().await;
            s.get("queue.mode")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        if queue_mode.as_deref() == Some("interrupt") {
            state.acp.cancel(session_id.to_string()).await;
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
        let _ = state.event_tx.send(GatewayEvent::AgentStatus {
            agent_id: agent_id.to_string(),
            status: AgentStatus::Processing {
                session_id: session_id.to_string(),
            },
        });

        // Build progress callback that forwards events to gateway subscribers
        let event_tx = state.event_tx.clone();
        let runtime_settings = state.runtime_settings.clone();
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
        match state
            .acp
            .execute_session_with_progress(agent_handle.agent.clone(), incoming_msg, progress_cb)
            .await
        {
            Ok(mut outgoing) => {
                // Apply reasoning visibility filter
                let reasoning_vis = {
                    let s = state.runtime_settings.read().await;
                    s.get("reasoning.visibility")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                };
                if reasoning_vis.as_deref() == Some("off") {
                    outgoing.reasoning_content = None;
                }

                // Accumulate usage statistics
                if let Some(ref usage) = outgoing.usage {
                    let mut settings = state.runtime_settings.write().await;
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

                // Generate run_id for this agent execution (OpenClaw-style run tracking)
                let run_id = uuid::Uuid::new_v4().to_string();

                // Save assistant response to persistent session history
                if let Some(ref store) = state.session_store {
                    let reasoning = outgoing.reasoning_content.as_deref();
                    let tool_calls_json = outgoing
                        .tool_calls
                        .as_ref()
                        .map(|calls| serde_json::to_string(calls).unwrap_or_default());
                    if let Err(e) = store
                        .append_message(
                            session_id,
                            "assistant",
                            &outgoing.content,
                            None,
                            reasoning,
                            tool_calls_json.as_deref(),
                            Some(session_id), // transcript_id defaults to session_id
                            Some(&run_id),
                        )
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
                                let _ = state.event_tx.send(GatewayEvent::SessionRenamed {
                                    session_id: session_id.to_string(),
                                    name: name.clone(),
                                });
                            }
                        }
                    }
                }
                let _ = state.event_tx.send(GatewayEvent::AgentResponse {
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
                let _ = state.event_tx.send(GatewayEvent::ProcessingError {
                    session_id: session_id.to_string(),
                    agent_id: agent_id.to_string(),
                    message: format!("Execution failed: {}", e),
                });
            }
        }

        let _ = state.event_tx.send(GatewayEvent::AgentStatus {
            agent_id: agent_id.to_string(),
            status: AgentStatus::Idle,
        });
    }

    #[allow(dead_code)]
    /// Start Tailscale for remote access
    async fn start_tailscale(&self) -> crate::Result<()> {
        #[cfg(feature = "tailscale")]
        {
            info!("Starting Tailscale integration...");
            crate::tailscale::start(self.config.port, self.config.tailscale_domain.clone()).await?;
        }

        #[cfg(not(feature = "tailscale"))]
        {
            warn!(
                "Tailscale feature not compiled in. Install with: cargo build --features tailscale"
            );
        }

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
                        let channels = state.channels.read().await;
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
                                let mut channels = state.channels.write().await;
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
                            let channels = state.channels.read().await;
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
                                        let mut channels = state.channels.write().await;
                                        channels.remove(name);
                                    }

                                    // Start with new config
                                    let gateway = Gateway {
                                        state: state.clone(),
                                        config: new_config.clone(),
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
                        let agents = state.agents.read().await;
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
                            let mut channels = state.channels.write().await;
                            if channels.remove(&channel_name).is_some() {
                                info!("✅ Stopped disabled channel '{}'", channel_name);
                            }
                            return Ok(());
                        }

                        // Stop existing channel
                        {
                            let mut channels = state.channels.write().await;
                            channels.remove(&channel_name);
                        }

                        // Re-initialize with new config
                        let gateway = Gateway {
                            state: state.clone(),
                            config: current_config.clone(),
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
                        match state.plugin_manager.reload_plugin(&plugin_id).await {
                            Ok(reloaded_id) => {
                                info!("✅ Reloaded plugin '{}' (preserved state)", reloaded_id);
                            }
                            Err(e) => {
                                warn!(
                                    "State-preserving reload failed for '{}', falling back to unload+load: {}",
                                    plugin_id, e
                                );
                                match state.plugin_manager.unload_plugin(&plugin_id).await {
                                    Ok(true) => {
                                        match state.plugin_manager.load_plugin(&plugin_dir).await
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
                                        match state.plugin_manager.load_plugin(&plugin_dir).await
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
                        info!(
                            "✅ Applied gateway config updates (security, providers, mcp settings)"
                        );

                        Ok(())
                    }
                })
                .await;
        }

        info!("Registered hot reload handlers for all config types");
    }
}






/// Create default tool registry with all built-in tools
async fn create_default_tool_registry(
    acp: Arc<AcpControlPlane>,
    mcp_manager: Arc<McpManager>,
    approval_queue: Arc<ApprovalQueue>,
    session_store: Option<Arc<crate::agent::session_store::SessionStore>>,
    memory_manager: Arc<
        tokio::sync::RwLock<Option<Arc<crate::memory::MemoryManager>>>,
    >,
) -> crate::Result<ToolRegistry> {
    use crate::tools::*;

    let mut registry = ToolRegistry::new().with_approval_queue(approval_queue);

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

    // Register OpenClaw-compatible session tools
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

    // Register delegation tool for agent-to-agent task delegation
    registry.register(Box::new(DelegateTool::root()));

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
        use crate::capabilities::{
            CapabilityProfile, CapabilityRegistry, ToolConflictStrategy,
        };

        let mut cap_reg = CapabilityRegistry::new();

        #[cfg(target_os = "linux")]
        {
            cap_reg.register(Box::new(crate::capabilities::LinuxSet::new()));
            cap_reg.register(Box::new(crate::capabilities::LinuxDesktopX11Set::new()));
            cap_reg.register(Box::new(crate::capabilities::LinuxDesktopWaylandSet::new()));
        }

        #[cfg(target_os = "macos")]
        {
            cap_reg.register(Box::new(crate::capabilities::MacosSet::new()));
        }

        // Apply profile (could be loaded from config in the future)
        let profile = CapabilityProfile::Full;
        profile.apply(&mut cap_reg);

        // Log detected capabilities before exporting
        let available = cap_reg.available_sets();
        if available.is_empty() {
            info!("No platform-specific capability sets detected on this host");
        } else {
            for set in &available {
                info!(
                    "Capability set available: {} ({}) — {}",
                    set.name(),
                    set.id(),
                    set.description()
                );
            }
        }

        cap_reg.export_to_tool_registry(&mut registry, ToolConflictStrategy::Reject);

        info!(
            "Capability sets exported: {} set(s) active",
            available.len()
        );
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
        state
            .agents
            .read()
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
            let mut records = state.repair_state.records.write().await;
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
        state.agents.write().await.remove(&agent_id);

        match spawn_agent_inner(state.clone(), agent_id.clone(), config).await {
            Ok(()) => {
                let mut records = state.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&agent_id));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
                info!("Agent {} restarted (attempt {})", agent_id, rec.restart_count);
                let _ = state.event_tx.send(GatewayEvent::RepairAction {
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
                let mut records = state.repair_state.records.write().await;
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

    let channels: Vec<(String, Arc<dyn Channel>)> = state
        .channels
        .read()
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
            let mut records = state.repair_state.records.write().await;
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
                let mut records = state.repair_state.records.write().await;
                let rec = records
                    .entry(key)
                    .or_insert_with(|| RepairRecord::new(&name));
                rec.restart_count += 1;
                rec.last_restart_at = Some(chrono::Utc::now());
                info!("Channel {} restarted (attempt {})", name, rec.restart_count);
                let _ = state.event_tx.send(GatewayEvent::RepairAction {
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
                let mut records = state.repair_state.records.write().await;
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
    state
        .repair_state
        .loop_running
        .store(true, Ordering::Relaxed);

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;
        *state.repair_state.last_cycle_at.write().await = Some(chrono::Utc::now());
        run_agent_watchdog_cycle(&state).await;
        run_channel_watchdog_cycle(&state).await;
    }
}






// ── Computer / Desktop Automation Handlers ─────────────────────────────────

/// Request body for executing a desktop action.
#[derive(Debug, Clone, Deserialize)]
pub struct ComputerExecuteRequest {
    action: crate::computer::DesktopAction,
}





/// Health report response structure
#[derive(Debug, Serialize)]
pub struct HealthReport {
    status: String,
    version: String,
    timestamp: String,
    overall_healthy: bool,
    subsystems: SubsystemHealth,
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

#[allow(dead_code)]
/// Simple chat handler for backwards compatibility with DaemonClient
#[derive(Debug, Deserialize)]
pub struct ChatRequestCompat {
    message: String,
    conversation_id: Option<String>,
}







#[allow(dead_code)]
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

#[allow(dead_code)]
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























#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct SetFallbackChainRequest {
    providers: Vec<String>,
}







// Vector Memory API Handlers

#[allow(dead_code)]
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


#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct MemoryAddRequest {
    content: String,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    collection: String,
}













#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct RunSkillRequest {
    /// Input for the skill
    input: String,
    /// Additional context
    #[serde(default)]
    context: Option<serde_json::Value>,
}



#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct SpawnSubagentRequest {
    task: String,
    #[serde(default = "default_acp_mode")]
    mode: String,
    #[serde(default)]
    agent_type: String,
}

fn default_acp_mode() -> String {
    "run".to_string()
}



#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AcpMessageRequest {
    message: String,
}








#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AcpExecuteRequest {
    message: String,
    user_id: String,
    agent_id: Option<String>,
}







#[allow(dead_code)]
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







#[allow(dead_code)]
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

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct SetSettingRequest {
    key: String,
    value: serde_json::Value,
}








#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct DenyApprovalRequest {
    reason: Option<String>,
}



#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AddCronJobRequest {
    name: String,
    schedule: String,
    command: String,
}








#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct CreateEntityRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    status: Option<String>,
}



#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct UpdateEntityRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}



#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SearchEntitiesRequest {
    query: String,
    #[serde(default)]
    entity_type: Option<String>,
}



#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ImportEntitiesRequest {
    entities: Vec<serde_json::Value>,
}



#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct CreateTeamRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
}





#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AddTeamMemberRequest {
    agent: String,
    #[serde(default = "default_member_role")]
    role: String,
}




#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AssignTeamTaskRequest {
    task: String,
    #[serde(default = "default_task_priority")]
    priority: String,
}















// ── Pairing / DM Access Control Handlers ───────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct PairingChannelQuery {
    channel: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ApprovePairingRequest {
    channel: String,
    code: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct RejectPairingRequest {
    channel: String,
    code: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct RevokePairingRequest {
    channel: String,
    user_id: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AddAllowlistRequest {
    channel: String,
    user_id: String,
    username: Option<String>,
}







// ── Command Gate Handlers ──────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct SetGateLevelRequest {
    user_id: String,
    level: String,
}




// ── Mention Gate Handlers ──────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct SetMentionPolicyRequest {
    policy: crate::security::mention_gate::MentionPolicy,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AddMentionPatternRequest {
    channel: String,
    pattern: String,
}









// ── Audit Log Handler ──────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    limit: Option<usize>,
    event_type: Option<String>,
}


#[cfg(test)]
mod api_tests;
#[cfg(test)]
pub(crate) mod state_tests;
