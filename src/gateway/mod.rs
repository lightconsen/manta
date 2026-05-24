//! Gateway Control Plane
//!
//! The Gateway is the control plane for Manta, managing:
//! - Multi-channel message routing (WhatsApp, Telegram, Feishu, etc.)
//! - Session management and routing to agents
//! - Agent spawning and lifecycle management
//! - WebSocket/HTTP API for channel adapters
//! - Authentication and security policies

// Transitional: management REST handlers are no longer routed (protocol.md v1.0
// Phase 3) but kept in source for reference during the migration window.
// They will be fully removed in Phase 5 cleanup.
#![allow(dead_code)]
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
pub enum EmbeddingProviderType {
    /// OpenAI API (requires API key)
    OpenAi,
    /// Local GGUF model (direct loading, no external service)
    LocalGguf,
}

impl Default for EmbeddingProviderType {
    fn default() -> Self {
        EmbeddingProviderType::OpenAi
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostGuardConfig {
    /// Maximum daily LLM spend in cents (0 = unlimited).
    /// Example: 500 = $5.00/day cap.
    #[serde(default)]
    pub daily_limit_cents: u64,
    /// Maximum provider calls per hour across all agents (0 = unlimited).
    #[serde(default)]
    pub hourly_action_limit: u64,
}

impl Default for CostGuardConfig {
    fn default() -> Self {
        Self {
            daily_limit_cents: 0,
            hourly_action_limit: 0,
        }
    }
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
    /// Memory manager — unified orchestrator with hybrid search (RwLock for late init)
    pub memory_manager: RwLock<Option<Arc<crate::memory::MemoryManager>>>,
    /// Hot reload manager for config changes (RwLock for late initialization)
    pub hot_reload: RwLock<Option<Arc<HotReloadManager>>>,
    /// Cron scheduler for scheduled jobs (RwLock for late initialization)
    pub cron_scheduler: RwLock<Option<Arc<tokio::sync::Mutex<crate::cron::cron::CronScheduler>>>>,
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

impl Gateway {
    /// Create a new gateway instance
    pub async fn new(config: GatewayConfig, config_path: Option<PathBuf>) -> crate::Result<Self> {
        let (event_tx, _) = broadcast::channel(1000);
        let (message_queue_tx, message_queue_rx) = mpsc::channel(1000);
        let (routed_tx, routed_rx) = mpsc::channel(1000);

        // Initialize storage adapter and shared SQLite pool early (needed for session_store → tool_registry)
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
                    .unwrap_or_else(|| crate::dirs::manta_dir().join("data").join("manta.db"));
                if let Some(parent) = db_path.parent() {
                    tokio::fs::create_dir_all(parent).await.ok();
                }
                if !db_path.exists() {
                    tokio::fs::File::create(&db_path).await.ok();
                }
                let db_url = format!("sqlite:///{}", db_path.display());
                info!("Connecting to SQLite storage at: {}", db_url);
                let pool = sqlx::SqlitePool::connect(&db_url).await.map_err(|e| {
                    crate::error::MantaError::Storage {
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

        // Create tool registry with built-in tools (including ACP tools if enabled)
        let tool_registry = Arc::new(
            create_default_tool_registry(
                acp.clone(),
                mcp_manager.clone(),
                approval_queue.clone(),
                session_store.clone(),
            )
            .await?,
        );

        // Initialize plugin manager
        let plugins_dir = crate::dirs::config_dir().join("plugins");
        let plugin_manager = {
            let pm = PluginManager::new(plugins_dir).await?;
            pm.set_tool_registry(tool_registry.clone()).await;
            Arc::new(pm)
        };

        // Create model router config with custom model settings
        let mut model_router_config = crate::model_router::ModelRouterConfig::default();
        model_router_config.default_model = "default".to_string();
        // Update the default alias to use the configured model and provider
        if let Some(default_alias) = model_router_config.aliases.get_mut("default") {
            default_alias.provider = config.model_provider.clone();
            default_alias.model = config.model.clone();
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

        // Configure ACP default agent builder (needs provider + tools, which are now ready)
        if let Ok(default_provider) = model_router.create_default_provider().await {
            let mut default_agent_config = config.default_agent.clone();
            default_agent_config.workspace_dir = config
                .workspace_dir
                .as_ref()
                .map(|d| crate::dirs::resolve_tilde(d));
            default_agent_config.workspace_only = config.workspace_only;
            let default_tools = tool_registry.clone();
            let provider_clone = default_provider.clone();
            let model_router_clone = model_router.clone();
            let default_model = config.model.clone();
            acp.set_agent_builder(move || {
                crate::agent::AgentBuilder::new()
                    .config(default_agent_config.clone())
                    .provider(provider_clone.clone())
                    .tools(default_tools.clone())
                    .model_router(model_router_clone.clone())
                    .model_alias(default_model.clone())
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
                    .map_err(|e| crate::error::MantaError::Storage {
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
            ));

        // Create state with placeholder values for vector_memory and hot_reload
        // We'll fill them in after state creation to allow callbacks to reference state
        let state = Arc::new(GatewayState {
            config: Arc::new(RwLock::new(config.clone())),
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
            hook_registry: Arc::new(hooks::EventHookRegistry::new()),
            message_queue: message_queue_tx,
            canvas_manager: Arc::new(CanvasManager::new()),
            plugin_manager,
            acp,
            vector_memory: RwLock::new(None),
            session_search: RwLock::new(None),
            memory_manager: RwLock::new(None),
            hot_reload: RwLock::new(None),
            cron_scheduler: RwLock::new(None),
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
            skills_manager: Arc::new(RwLock::new(crate::skills::SkillManager::new().await?)),
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
                let _ = store.init();
                Arc::new(store)
            },
            artifact_store: {
                let store = crate::agent::ArtifactStore::new(crate::dirs::artifacts_dir());
                let _ = store.init();
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
                            .map_err(|e| crate::error::MantaError::Storage {
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
                            .map_err(|e| crate::error::MantaError::Storage {
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

    /// Start the gateway
    pub async fn start(&self) -> crate::Result<()> {
        info!("Starting Manta Gateway control plane...");

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
        match self
            .spawn_agent("default".to_string(), self.config.default_agent.clone())
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

        // Start dream scheduler if memory manager is available
        if let Some(ref mm) = *self.state.memory_manager.read().await {
            if let Some(tier_index) = mm.tier_index() {
                let dream_config = crate::memory::DreamConfig::default();
                let tier_system_config = crate::memory::TierSystemConfig::default();
                let mut engine = crate::memory::DreamEngine::new(dream_config, tier_system_config);
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
            }
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
            crate::error::MantaError::ExternalService {
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

        // Run the server
        axum::serve(listener, app).await.map_err(|e| {
            crate::error::MantaError::ExternalService {
                source: "Gateway server error".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

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
        let essential_router = Router::new()
            // Health checks (public)
            .route("/health", get(health_handler))
            .route("/ready", get(ready_handler))
            .route("/live", get(live_handler))
            // WebSocket endpoints (localhost/Tailscale only)
            .route("/ws", get(ws::ws_handler))
            .route("/ws/canvas/:id", get(canvas_ws_handler))
            // OpenAI-compatible API
            .route("/v1/chat/completions", post(openai_chat_completions_handler))
            .route("/v1/models", get(openai_list_models_handler))
            // Manta as MCP server – Streamable-HTTP endpoint
            .route("/mcp", post(manta_as_mcp_server_handler))
            // Admin redirect — management UI moved to CLI
            .route("/admin", get(admin_redirect_handler));

        // Apply security middleware to essential router
        // (order matters - applied in reverse)
        let admin_router = essential_router
            .layer(from_fn_with_state(state.clone(), middleware::rate_limit_middleware))
            .layer(from_fn_with_state(state.clone(), middleware::auth_middleware))
            .layer(from_fn_with_state(state.clone(), auth::session_cookie_middleware))
            .layer(from_fn(middleware::tailscale_only_middleware))
            .layer(from_fn(middleware::security_headers_middleware))
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

        // SPA frontend routes (serve built React app from web/dist/)
        let frontend_router = Router::new()
            .route("/", get(web_terminal_html_handler))
            .route("/favicon.svg", get(favicon_handler))
            .nest_service("/assets", tower_http::services::ServeDir::new("web/dist/assets"));

        // Merge all routers and apply global CORS
        frontend_router
            .merge(public_router)
            .merge(auth_router)
            .merge(admin_router)
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
    config: AgentConfig,
) -> crate::Result<()> {
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
    let agent = if let Some(ref mm) = memory_manager {
        let chat_history = mm.chat_history();
        Arc::new(
            Agent::new(config.clone(), provider, tools)
                .with_model(model.clone())
                .with_memory_manager(mm.clone())
                .with_chat_history(chat_history)
                .with_cost_guard(cost_guard)
                .with_transcript_store(Arc::clone(&state.transcript_store))
                .with_artifact_store(Arc::clone(&state.artifact_store))
                .with_disk_budget(Arc::clone(&state.disk_budget))
                .with_session_file_manager(Arc::clone(&state.session_file_manager))
                .with_model_router(Arc::clone(&state.model_router))
                .with_model_alias(model.clone()),
        )
    } else {
        Arc::new(
            Agent::new(config.clone(), provider, tools)
                .with_model(model.clone())
                .with_cost_guard(cost_guard)
                .with_transcript_store(Arc::clone(&state.transcript_store))
                .with_artifact_store(Arc::clone(&state.artifact_store))
                .with_disk_budget(Arc::clone(&state.disk_budget))
                .with_session_file_manager(Arc::clone(&state.session_file_manager))
                .with_model_router(Arc::clone(&state.model_router))
                .with_model_alias(model.clone()),
        )
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
                        model_override,
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
                    return Err(crate::error::MantaError::Validation(format!(
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
            ChannelType::Feishu | ChannelType::WebTerminal => {
                // Feishu/Lark and WebTerminal are handled via webhooks/SocketMode
                info!(
                    "Channel '{}' ({:?}) uses webhook/SocketMode, skipping adapter spawn",
                    name, config.channel_type
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
            let discord_config = crate::channels::discord::DiscordConfig::new(token);

            let channel = Arc::new(crate::channels::discord::DiscordChannel::new(discord_config));
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
            let slack_config = crate::channels::slack::SlackConfig::new(token);

            let channel = Arc::new(crate::channels::slack::SlackChannel::new(slack_config));
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

    /// Initialize QQ channel
    #[cfg(feature = "qq")]
    async fn init_qq_channel(&self, name: &str, config: &ChannelConfig) -> crate::Result<()> {
        if let (Some(app_id), Some(app_secret), Some(bot_qq)) = (
            config.credentials.get("app_id"),
            config.credentials.get("app_secret"),
            config.credentials.get("bot_qq"),
        ) {
            let qq_config = crate::channels::qq::QqConfig::new(app_id, app_secret, bot_qq);

            let channel = Arc::new(crate::channels::qq::QqChannel::new(qq_config));
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
                    crate::agent::ProgressEvent::ToolResult { name, result } => {
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

        // Handler for main config changes (includes manta.toml)
        hot_reload
            .register_handler(ConfigFileType::Main, move |_event| {
                let state = state.clone();
                let current_config = current_config.clone();
                async move {
                    info!("Main config file changed - reloading configuration");

                    // Reload config from disk
                    let config_path = crate::dirs::manta_dir().join("manta.toml");
                    if !config_path.exists() {
                        return Ok(());
                    }

                    let content = match tokio::fs::read_to_string(&config_path).await {
                        Ok(c) => c,
                        Err(e) => {
                            error!("Failed to read manta.toml: {}", e);
                            return Ok(());
                        }
                    };

                    let new_config: GatewayConfig = match toml::from_str(&content) {
                        Ok(cfg) => cfg,
                        Err(e) => {
                            error!("Failed to parse manta.toml: {}", e);
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

/// HTML handler for the web chat UI
///
/// Serves the built React app from `web/dist/index.html`.
async fn web_terminal_html_handler() -> Html<String> {
    let html = tokio::fs::read_to_string("web/dist/index.html")
        .await
        .unwrap_or_else(|_| {
            format!(
                "<h1>Manta Chat UI</h1><p>Build not found. Run: cd web/chat-ui and pnpm build</p>"
            )
        });
    Html(html.replace("{VERSION}", crate::VERSION))
}

/// Favicon handler — serves the manta ray SVG favicon
async fn favicon_handler() -> impl IntoResponse {
    let svg = tokio::fs::read_to_string("web/dist/favicon.svg")
        .await
        .unwrap_or_else(|_| {
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 80"><path d="M50 8C50 8 38 0 28 8C18 16 8 24 2 36C-2 44 2 52 10 48C18 44 22 40 26 36C30 32 34 28 38 30C42 32 44 38 44 46C44 54 42 64 40 72C38 76 42 78 44 74C46 66 48 56 50 50C52 56 54 66 56 74C58 78 62 76 60 72C58 64 56 54 56 46C56 38 58 32 62 30C66 28 70 32 74 36C78 40 82 44 90 48C98 52 102 44 98 36C92 24 82 16 72 8C62 0 50 8 50 8Z" fill="#10b981"/><circle cx="38" cy="18" r="2" fill="white"/><circle cx="62" cy="18" r="2" fill="white"/></svg>"##.to_string()
        });
    ([(header::CONTENT_TYPE, "image/svg+xml")], svg)
}

/// Admin redirect handler — admin UI moved to CLI
async fn admin_redirect_handler() -> Html<&'static str> {
    Html("<h1>Admin UI Moved</h1><p>Administration is now available via CLI: <code>manta admin</code></p>")
}

/// Create default tool registry with all built-in tools
async fn create_default_tool_registry(
    acp: Arc<AcpControlPlane>,
    mcp_manager: Arc<McpManager>,
    approval_queue: Arc<ApprovalQueue>,
    session_store: Option<Arc<crate::agent::session_store::SessionStore>>,
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

    // Register nodes/Tailscale tool
    registry.register(Box::new(NodesTool::new()));

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
            } else if rec
                .last_restart_at
                .map(|t| (chrono::Utc::now() - t).num_seconds() < COOLDOWN_SECS)
                .unwrap_or(false)
            {
                false
            } else {
                true
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
            } else if rec
                .last_restart_at
                .map(|t| (chrono::Utc::now() - t).num_seconds() < COOLDOWN_SECS)
                .unwrap_or(false)
            {
                false
            } else {
                true
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

// HTTP Handlers

/// Comprehensive health check with all subsystem statuses.
/// Returns 200 if healthy, 503 if any critical subsystem is down.
async fn health_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let report = build_health_report(&state).await;
    let status_code = if report.overall_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, Json(report))
}

/// Readiness probe — returns 200 when the gateway is ready to serve traffic.
/// Checks: agents, providers, channels.
async fn ready_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let agents = state.agents.read().await;
    let agent_ready = agents.get("default").is_some();
    let agent_count = agents.len();
    drop(agents);

    let router_health = state.model_router.get_health_status().await;
    let healthy_providers = router_health
        .values()
        .filter(|h| matches!(h.state, crate::model_router::CircuitState::Closed))
        .count();

    let channels = state.channels.read().await;
    let channel_count = channels.len();
    drop(channels);

    let ready = agent_ready && healthy_providers > 0 && channel_count > 0;

    let status_code = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status_code,
        Json(serde_json::json!({
            "ready": ready,
            "agents": { "ready": agent_ready, "count": agent_count },
            "providers": { "healthy": healthy_providers, "total": router_health.len() },
            "channels": { "count": channel_count },
        })),
    )
}

/// Liveness probe — returns 200 if the gateway process is alive.
/// Lightweight check that just confirms the process is running.
async fn live_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "alive": true,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        })),
    )
}

/// Build a comprehensive health report covering all subsystems.
async fn build_health_report(state: &Arc<GatewayState>) -> HealthReport {
    // Agents
    let agents = state.agents.read().await;
    let agent_ready = agents.get("default").is_some();
    let agent_count = agents.len();
    drop(agents);

    // Providers
    let router_health = state.model_router.get_health_status().await;
    let healthy_providers = router_health
        .values()
        .filter(|h| matches!(h.state, crate::model_router::CircuitState::Closed))
        .count();
    let total_providers = router_health.len();

    // Channels
    let channels = state.channels.read().await;
    let channel_count = channels.len();
    drop(channels);

    // Vector memory
    let vector_memory_ready = state.vector_memory.read().await.is_some();

    // Memory manager
    let memory_manager_ready = state.memory_manager.read().await.is_some();

    // Cron scheduler
    let cron_ready = state.cron_scheduler.read().await.is_some();

    // Plugins
    let plugin_count = state.plugin_manager.list_plugins().await.len();

    // MCP servers
    let mcp_count = state.mcp_manager.list_servers().await.len();

    // Storage
    let storage_healthy = state.storage.read().await.health_check().await.is_ok();

    // Cost guard
    let cost_exceeded = state.cost_guard.is_exceeded();
    let daily_spend = state.cost_guard.daily_spend_cents() as f64 / 100.0;

    // Overall: agents + providers are critical; others are warnings
    let overall_healthy = agent_ready && healthy_providers > 0;

    HealthReport {
        status: if overall_healthy {
            "healthy".to_string()
        } else {
            "degraded".to_string()
        },
        version: crate::VERSION.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        overall_healthy,
        subsystems: SubsystemHealth {
            agents: HealthStatus {
                healthy: agent_ready,
                message: format!("{} agents active", agent_count),
            },
            providers: HealthStatus {
                healthy: healthy_providers > 0,
                message: format!("{}/{} healthy", healthy_providers, total_providers),
            },
            channels: HealthStatus {
                healthy: channel_count > 0,
                message: format!("{} channels configured", channel_count),
            },
            vector_memory: HealthStatus {
                healthy: vector_memory_ready,
                message: if vector_memory_ready {
                    "ready".to_string()
                } else {
                    "not initialized".to_string()
                },
            },
            memory_manager: HealthStatus {
                healthy: memory_manager_ready,
                message: if memory_manager_ready {
                    "ready".to_string()
                } else {
                    "not initialized".to_string()
                },
            },
            cron: HealthStatus {
                healthy: cron_ready,
                message: if cron_ready {
                    "running".to_string()
                } else {
                    "not initialized".to_string()
                },
            },
            plugins: HealthStatus {
                healthy: true,
                message: format!("{} plugins loaded", plugin_count),
            },
            mcp: HealthStatus {
                healthy: mcp_count > 0,
                message: format!("{} MCP servers connected", mcp_count),
            },
            storage: HealthStatus {
                healthy: storage_healthy,
                message: if storage_healthy {
                    "healthy".to_string()
                } else {
                    "unavailable".to_string()
                },
            },
            cost_guard: HealthStatus {
                healthy: !cost_exceeded,
                message: format!("${:.4} today", daily_spend),
            },
        },
    }
}

/// Health report response structure
#[derive(Debug, Serialize)]
struct HealthReport {
    status: String,
    version: String,
    timestamp: String,
    overall_healthy: bool,
    subsystems: SubsystemHealth,
}

/// Per-subsystem health statuses
#[derive(Debug, Serialize)]
struct SubsystemHealth {
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
struct HealthStatus {
    healthy: bool,
    message: String,
}

/// Simple chat handler for backwards compatibility with DaemonClient
#[derive(Debug, Deserialize)]
struct ChatRequestCompat {
    message: String,
    conversation_id: Option<String>,
}

async fn chat_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<ChatRequestCompat>,
) -> impl IntoResponse {
    let conversation_id = body
        .conversation_id
        .unwrap_or_else(|| "default".to_string());

    // Use the default agent to process the message
    let agents = state.agents.read().await;
    if let Some(agent_handle) = agents.get("default") {
        // Subscribe to events before sending the command to avoid race condition
        let mut event_rx = state.event_tx.subscribe();

        // Send ProcessMessage command to agent
        let cmd = AgentCommand::ProcessMessage {
            session_id: conversation_id.clone(),
            message: body.message.clone(),
            user_id: "web_user".to_string(),
            channel: "web".to_string(),
            model_override: None,
        };

        if let Err(e) = agent_handle.tx.send(cmd).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to send message to agent: {}", e),
                })),
            );
        }

        // Drop the agents lock so we don't hold it while waiting
        drop(agents);

        // Wait for response with timeout
        let timeout = tokio::time::Duration::from_secs(120);
        let start = tokio::time::Instant::now();

        loop {
            // Check for timeout
            if start.elapsed() > timeout {
                return (
                    StatusCode::REQUEST_TIMEOUT,
                    Json(serde_json::json!({
                        "error": "Request timeout",
                    })),
                );
            }

            // Wait for event with a smaller timeout to allow checking
            match tokio::time::timeout(tokio::time::Duration::from_millis(100), event_rx.recv())
                .await
            {
                Ok(Ok(GatewayEvent::AgentResponse {
                    session_id,
                    agent_id: _,
                    content,
                    ..
                })) => {
                    if session_id == conversation_id {
                        let resp = serde_json::json!({
                            "response": content,
                            "conversation_id": conversation_id,
                        });
                        return (StatusCode::OK, Json(resp));
                    }
                    // Not our session, continue waiting
                }
                Ok(Ok(_)) => {
                    // Some other event, continue waiting
                    continue;
                }
                Ok(Err(_)) => {
                    // Event channel closed
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "Event channel closed",
                        })),
                    );
                }
                Err(_) => {
                    // Timeout on recv, continue loop to check overall timeout
                    continue;
                }
            }
        }
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "No default agent available",
            })),
        )
    }
}

async fn list_agents_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    // Get running agents
    let running_agents = state.agents.read().await;

    // Get discovered personalities from registry
    let registry = state.agent_registry.read().await;
    let discovered: Vec<_> = registry.iter().map(|p| p.id.clone()).collect();

    let list: Vec<_> = running_agents
        .iter()
        .map(|(id, handle)| {
            let is_discovered = discovered.contains(id);
            serde_json::json!({
                "id": id,
                "busy": handle.busy,
                "status": "running",
                "discovered": is_discovered,
            })
        })
        .collect();

    // Add discovered but not running agents
    let not_running: Vec<_> = discovered
        .into_iter()
        .filter(|id| !running_agents.contains_key(id))
        .map(|id| {
            serde_json::json!({
                "id": id,
                "busy": false,
                "status": "discovered",
                "discovered": true,
            })
        })
        .collect();

    let combined: Vec<_> = list.into_iter().chain(not_running).collect();
    Json(combined)
}

async fn create_agent_handler(
    State(state): State<Arc<GatewayState>>,
    Json(config): Json<AgentConfig>,
) -> impl IntoResponse {
    use crate::agent::Agent;
    use tracing::info;

    // Generate unique agent ID
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4());
    info!("Creating new agent via API: {}", agent_id);

    // Create communication channel
    let (tx, mut rx) = mpsc::channel(100);

    // Create provider from model router
    let provider = match state.model_router.create_default_provider().await {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to create provider: {}", e)
                })),
            )
                .into_response();
        }
    };

    // Get tools, model, and memory manager
    let tools = state.tool_registry.clone();
    let model = state.config.read().await.model.clone();
    let memory_manager = state.memory_manager.read().await.clone();

    // Create agent instance with memory manager and session management stores
    let agent = if let Some(mm) = memory_manager {
        Arc::new(
            Agent::new(config.clone(), provider, tools)
                .with_model(model)
                .with_memory_manager(mm)
                .with_transcript_store(Arc::clone(&state.transcript_store))
                .with_artifact_store(Arc::clone(&state.artifact_store))
                .with_disk_budget(Arc::clone(&state.disk_budget))
                .with_session_file_manager(Arc::clone(&state.session_file_manager)),
        )
    } else {
        Arc::new(
            Agent::new(config.clone(), provider, tools)
                .with_model(model)
                .with_transcript_store(Arc::clone(&state.transcript_store))
                .with_artifact_store(Arc::clone(&state.artifact_store))
                .with_disk_budget(Arc::clone(&state.disk_budget))
                .with_session_file_manager(Arc::clone(&state.session_file_manager)),
        )
    };

    let (query_tx, mut query_rx) = mpsc::channel::<AgentQuery>(32);

    // Create agent handle
    let handle = AgentHandle {
        id: agent_id.clone(),
        config: config.clone(),
        tx: tx.clone(),
        query_tx: query_tx.clone(),
        busy: false,
        agent: agent.clone(),
    };

    // Insert into agents map
    {
        let mut agents = state.agents.write().await;
        agents.insert(agent_id.clone(), handle);
    }

    // Start agent processing loop (mirrors spawn_agent behavior)
    let state_clone = state.clone();
    let agent_id_clone = agent_id.clone();
    let agent_clone = agent.clone();
    tokio::spawn(async move {
        info!("Agent {} processing loop started", agent_id_clone);
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                let cmd = match cmd { Some(c) => c, None => break };
                match cmd {
                    AgentCommand::ProcessMessage { session_id, message, user_id, channel, model_override } => {
                        let source_channel = channel;
                        info!("Agent {} processing message for session {}", agent_id_clone, session_id);

                        // Update status
                        let _ = state_clone.event_tx.send(GatewayEvent::AgentStatus {
                            agent_id: agent_id_clone.clone(),
                            status: AgentStatus::Processing { session_id: session_id.clone() },
                        });

                        // Create incoming message
                        let incoming_msg = crate::channels::IncomingMessage::new(
                            user_id.clone(), session_id.clone(), message.clone()
                        );

                        // Process with progress callbacks
                        let progress_state = state_clone.clone();
                        let progress_session = session_id.clone();
                        let progress_agent = agent_id_clone.clone();
                        let progress_cb: crate::agent::ProgressCallback = Arc::new(move |event| {
                            let state = progress_state.clone();
                            let sid = progress_session.clone();
                            let aid = progress_agent.clone();
                            Box::pin(async move {
                                // Read directive settings
                                let reasoning_vis = {
                                    let s = state.runtime_settings.read().await;
                                    s.get("reasoning.visibility").and_then(|v| v.as_str()).map(|s| s.to_string())
                                };
                                let verbose_mode = {
                                    let s = state.runtime_settings.read().await;
                                    s.get("verbose.mode").and_then(|v| v.as_str()).map(|s| s.to_string())
                                };
                                match event {
                                    crate::agent::ProgressEvent::Started => {
                                        let _ = state.event_tx.send(GatewayEvent::AgentStatus {
                                            agent_id: aid.clone(),
                                            status: AgentStatus::Processing { session_id: sid.clone() },
                                        });
                                    }
                                    crate::agent::ProgressEvent::Generating { content } => {
                                        if reasoning_vis.as_deref() == Some("off") {
                                            return;
                                        }
                                        // Only emit thinking events when there's actual content
                                        if let Some(ref thinking) = content {
                                            if !thinking.is_empty() {
                                                let _ = state.event_tx.send(GatewayEvent::Thinking {
                                                    session_id: sid.clone(),
                                                    agent_id: aid.clone(),
                                                    content: Some(thinking.clone()),
                                                });
                                            }
                                        }
                                    }
                                    crate::agent::ProgressEvent::ContentDelta { text } => {
                                        let _ = state.event_tx.send(GatewayEvent::ContentDelta {
                                            session_id: sid.clone(),
                                            agent_id: aid.clone(),
                                            delta: text,
                                        });
                                    }
                                    crate::agent::ProgressEvent::ToolCalling { name, arguments } => {
                                        if verbose_mode.as_deref() == Some("off") {
                                            return;
                                        }
                                        let _ = state.event_tx.send(GatewayEvent::ToolCalling {
                                            session_id: sid.clone(), agent_id: aid.clone(),
                                            tool_name: name.clone(), arguments: arguments.clone(),
                                        });
                                    }
                                    crate::agent::ProgressEvent::ToolResult { name, result } => {
                                        if verbose_mode.as_deref() == Some("off") {
                                            return;
                                        }
                                        let result = if verbose_mode.as_deref() == Some("compact") {
                                            if result.len() > 500 {
                                                format!("{}... (truncated)", &result[..500])
                                            } else {
                                                result
                                            }
                                        } else {
                                            result
                                        };
                                        let _ = state.event_tx.send(GatewayEvent::ToolResult {
                                            session_id: sid.clone(), agent_id: aid.clone(),
                                            tool_name: name.clone(), result,
                                        });
                                    }
                                    crate::agent::ProgressEvent::Completed { response } => {
                                        let _ = state.event_tx.send(GatewayEvent::Completed {
                                            session_id: sid.clone(),
                                            agent_id: aid.clone(),
                                            response,
                                        });
                                    }
                                    crate::agent::ProgressEvent::Error { message } => {
                                        let _ = state.event_tx.send(GatewayEvent::ProcessingError {
                                            session_id: sid.clone(),
                                            agent_id: aid.clone(),
                                            message,
                                        });
                                    }
                                }
                            })
                        });

                        // Apply thinking config from runtime settings
                        let think_level = {
                            let s = state_clone.runtime_settings.read().await;
                            s.get("think.level").and_then(|v| v.as_str()).map(|s| s.to_string())
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
                        agent_clone.set_model_override(model_override).await;
                        agent_clone.set_extra_params(extra).await;

                        let result = agent_clone.process_message_with_progress(incoming_msg, progress_cb).await;
                        agent_clone.set_model_override(None).await;

                        match result {
                            Ok(mut outgoing) => {
                                // Apply reasoning visibility filter
                                let reasoning_vis = {
                                    let s = state_clone.runtime_settings.read().await;
                                    s.get("reasoning.visibility").and_then(|v| v.as_str()).map(|s| s.to_string())
                                };
                                if reasoning_vis.as_deref() == Some("off") {
                                    outgoing.reasoning_content = None;
                                }
                                // Accumulate usage
                                if let Some(ref usage) = outgoing.usage {
                                    let mut settings = state_clone.runtime_settings.write().await;
                                    let current_tokens = settings.get("usage.tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                    let total_tokens = usage.prompt_tokens as u64 + usage.completion_tokens as u64;
                                    settings.insert("usage.tokens".to_string(), serde_json::json!(current_tokens + total_tokens));
                                    let current_calls = settings.get("usage.calls").and_then(|v| v.as_u64()).unwrap_or(0);
                                    let tool_calls = outgoing.tool_calls.as_ref().map(|c| c.len() as u64).unwrap_or(0);
                                    settings.insert("usage.calls".to_string(), serde_json::json!(current_calls + tool_calls + 1));
                                }
                                let _ = state_clone.event_tx.send(GatewayEvent::AgentResponse {
                                    session_id: session_id.clone(), agent_id: agent_id_clone.clone(),
                                    content: outgoing.content, channel: source_channel.clone(),
                                    conversation_id: session_id.clone(), usage: outgoing.usage,
                                });
                            }
                            Err(e) => {
                                error!("Agent {} failed to process: {}", agent_id_clone, e);
                            }
                        }

                        let _ = state_clone.event_tx.send(GatewayEvent::AgentStatus {
                            agent_id: agent_id_clone.clone(), status: AgentStatus::Idle,
                        });
                    }
                    AgentCommand::Shutdown => {
                        info!("Agent {} shutting down", agent_id_clone);
                        let _ = state_clone.event_tx.send(GatewayEvent::AgentStatus {
                            agent_id: agent_id_clone.clone(), status: AgentStatus::Shutdown,
                        });
                        break;
                    }
                    _ => info!("Agent {} received command: {:?}", agent_id_clone, cmd),
                }
                } // cmd = rx.recv() arm
                query = query_rx.recv() => {
                    let query = match query { Some(q) => q, None => break };
                    match query {
                        AgentQuery::GetThreadSummaries { response_tx } => {
                            let _ = response_tx.send(agent_clone.thread_summaries().await);
                        }
                        AgentQuery::GetThreadTurns { conv_id, response_tx } => {
                            let _ = response_tx.send(agent_clone.thread_turns_for(&conv_id).await);
                        }
                        AgentQuery::UndoLastTurn { conv_id, response_tx } => {
                            let _ = response_tx.send(agent_clone.undo_last_turn(&conv_id).await);
                        }
                        AgentQuery::RedoLastTurn { conv_id, response_tx } => {
                            let _ = response_tx.send(agent_clone.redo_last_turn(&conv_id).await);
                        }
                        AgentQuery::RunSkill { session_id, message, user_id, skill_trust, response_tx } => {
                            agent_clone.set_skill_trust(skill_trust);
                            let incoming = crate::channels::IncomingMessage::new(
                                user_id, &session_id, message,
                            );
                            let no_op: crate::agent::ProgressCallback =
                                Arc::new(|_| Box::pin(async {}));
                            let result =
                                agent_clone.process_message_with_progress(incoming, no_op).await;
                            agent_clone.set_skill_trust(crate::tools::SkillTrust::Trusted);
                            let _ = response_tx.send(result);
                        }
                    }
                }
            }
        } // end tokio::select! and loop
        info!("Agent {} processing loop ended", agent_id_clone);
    });

    info!("✅ Agent {} created successfully", agent_id);
    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": agent_id,
            "status": "created",
            "config": {
                "max_context_tokens": config.max_context_tokens,
                "max_concurrent_tools": config.max_concurrent_tools,
                "temperature": config.temperature,
                "max_tokens": config.max_tokens,
            }
        })),
    )
        .into_response()
}

async fn get_agent_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let agents = state.agents.read().await;
    match agents.get(&id) {
        Some(agent) => Json(serde_json::json!({
            "id": agent.id,
            "busy": agent.busy,
        }))
        .into_response(),
        None => (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    }
}

async fn delete_agent_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    use tracing::{info, warn};

    info!("Deleting agent via API: {}", id);

    // Check if agent exists
    let agent_exists = {
        let agents = state.agents.read().await;
        agents.contains_key(&id)
    };

    if !agent_exists {
        warn!("Agent {} not found for deletion", id);
        return StatusCode::NOT_FOUND;
    }

    // Get the agent's channel and send shutdown
    let tx = {
        let agents = state.agents.read().await;
        agents.get(&id).map(|h| h.tx.clone())
    };

    if let Some(tx) = tx {
        // Send shutdown command
        if let Err(e) = tx.send(AgentCommand::Shutdown).await {
            warn!("Failed to send shutdown to agent {}: {}", id, e);
        }
    }

    // Remove from agents map
    {
        let mut agents = state.agents.write().await;
        agents.remove(&id);
    }

    // Send event
    let _ = state.event_tx.send(GatewayEvent::AgentStatus {
        agent_id: id.clone(),
        status: AgentStatus::Shutdown,
    });

    info!("✅ Agent {} deleted successfully", id);
    StatusCode::NO_CONTENT
}

async fn list_channels_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let channels = state.channels.read().await;
    let list: Vec<_> = channels.keys().cloned().collect();
    Json(list)
}

/// Request body for web terminal chat
#[derive(Debug, Deserialize)]
struct WebTerminalChatRequest {
    /// Message content from user
    message: String,
    /// Optional conversation ID (creates new if not provided)
    conversation_id: Option<String>,
    /// Optional user ID
    user_id: Option<String>,
}

/// Response for web terminal chat
#[derive(Debug, Serialize)]
struct WebTerminalChatResponse {
    /// Message ID
    message_id: String,
    /// Conversation ID (new or existing)
    conversation_id: String,
    /// Status
    status: String,
}

/// `POST /api/chat` — Send a message from the web terminal.
///
/// The message is queued for processing and a 202 Accepted is returned immediately.
/// The actual response(s) will be streamed via SSE on `GET /api/events`.
async fn web_terminal_chat_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<WebTerminalChatRequest>,
) -> impl IntoResponse {
    let message_id = uuid::Uuid::new_v4().to_string();
    let user_id = body.user_id.unwrap_or_else(|| "web_user".to_string());
    let conversation_id = body
        .conversation_id
        .unwrap_or_else(|| AgentRouter::derive_session_key("web", &user_id));

    // Access control check
    if let Err(reason) = state
        .check_incoming_access(
            "web",
            &user_id,
            &body.message,
            &crate::channels::MentionState::DirectMessage,
        )
        .await
    {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": reason })))
            .into_response();
    }

    // Route through inbound pipeline
    let incoming =
        crate::channels::IncomingMessage::new(user_id, conversation_id.clone(), body.message)
            .with_provenance(crate::channels::InputProvenance::ExternalUser {
                channel: "web".to_string(),
                is_direct: true,
            });
    let _ = state.inbound_pipeline.process(incoming).await;

    let resp = WebTerminalChatResponse {
        message_id,
        conversation_id,
        status: "processing".to_string(),
    };
    (StatusCode::ACCEPTED, Json(resp)).into_response()
}

async fn send_message_handler(
    State(state): State<Arc<GatewayState>>,
    Path(session_id): Path<String>,
    Json(body): Json<SendMessageRequest>,
) -> impl IntoResponse {
    // Check if provider override is specified
    let provider_override = body.provider_override.clone();

    // Queue message for processing with provider override
    let message_id = uuid::Uuid::new_v4().to_string();
    let user_id = body
        .user_id
        .clone()
        .unwrap_or_else(|| "api_user".to_string());

    // Access control check
    if let Err(reason) = state
        .check_incoming_access(
            "api",
            &user_id,
            &body.message,
            &crate::channels::MentionState::DirectMessage,
        )
        .await
    {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({ "error": reason })))
            .into_response();
    }

    // If provider override is specified, we route through that provider
    if let Some(provider_name) = provider_override {
        match state
            .model_router
            .complete_with_provider(
                &provider_name,
                body.model_id,
                vec![crate::providers::Message::user(body.message.clone())],
            )
            .await
        {
            Ok(response) => {
                let resp = serde_json::json!({
                    "message_id": message_id,
                    "session_id": session_id,
                    "provider_override": provider_name,
                    "response": response.message.content,
                    "status": "completed",
                });
                return (StatusCode::OK, Json(resp)).into_response();
            }
            Err(e) => {
                let resp = serde_json::json!({
                    "message_id": message_id,
                    "session_id": session_id,
                    "error": format!("Provider override failed: {}", e),
                    "status": "failed",
                });
                return (StatusCode::BAD_REQUEST, Json(resp)).into_response();
            }
        }
    }

    // Otherwise, route through inbound pipeline for normal agent processing
    let incoming = crate::channels::IncomingMessage::new(user_id, session_id.clone(), body.message)
        .with_provenance(crate::channels::InputProvenance::ExternalUser {
            channel: "api".to_string(),
            is_direct: true,
        });
    let _ = state.inbound_pipeline.process(incoming).await;

    let resp = serde_json::json!({
        "message_id": message_id,
        "session_id": session_id,
        "queued": true,
        "status": "processing",
    });
    (StatusCode::ACCEPTED, Json(resp)).into_response()
}

/// Get conversation history
async fn get_conversation_history_handler(
    State(state): State<Arc<GatewayState>>,
    Path(conversation_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let limit: usize = params
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(100);

    // Access storage directly to get chat history
    let storage = state.storage.read().await;

    match storage
        .get_conversation_history(&conversation_id, limit)
        .await
    {
        Ok(messages) => {
            let messages_json: Vec<_> = messages
                .into_iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "conversation_id": m.conversation_id,
                        "user_id": m.user_id,
                        "role": m.role,
                        "content": m.content,
                        "created_at": m.created_at,
                    })
                })
                .collect();

            let resp = serde_json::json!({
                "conversation_id": conversation_id,
                "messages": messages_json,
            });
            (StatusCode::OK, Json(resp))
        }
        Err(e) => {
            error!("Failed to get conversation history: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to get conversation history: {}", e)
                })),
            )
        }
    }
}

/// Get last conversation for a user
async fn get_last_conversation_handler(
    State(state): State<Arc<GatewayState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let user_id = params
        .get("user_id")
        .cloned()
        .unwrap_or_else(|| "web_user".to_string());

    // Access storage directly to get last conversation
    let storage = state.storage.read().await;

    match storage.get_last_conversation(&user_id).await {
        Ok(conversation_id) => {
            let resp = serde_json::json!({
                "conversation_id": conversation_id,
                "user_id": user_id,
            });
            (StatusCode::OK, Json(resp))
        }
        Err(e) => {
            error!("Failed to get last conversation: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to get last conversation: {}", e)
                })),
            )
        }
    }
}

/// List all conversations for a user
async fn list_conversations_handler(
    State(state): State<Arc<GatewayState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let user_id = params
        .get("user_id")
        .cloned()
        .unwrap_or_else(|| "web_user".to_string());

    let storage = state.storage.read().await;

    match storage.get_user_conversations(&user_id, 100).await {
        Ok(conversation_ids) => {
            let conversations: Vec<serde_json::Value> = conversation_ids
                .into_iter()
                .map(|id| serde_json::json!({"id": id}))
                .collect();

            let resp = serde_json::json!({
                "conversations": conversations,
                "user_id": user_id,
            });
            (StatusCode::OK, Json(resp))
        }
        Err(e) => {
            error!("Failed to list conversations: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("Failed to list conversations: {}", e)
                })),
            )
        }
    }
}

async fn status_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let agents = state.agents.read().await;
    let channels = state.channels.read().await;

    Json(serde_json::json!({
        "agents": {
            "total": agents.len(),
            "busy": agents.values().filter(|a| a.busy).count(),
        },
        "channels": channels.len(),
        "version": crate::VERSION,
    }))
}

async fn repair_status_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    use std::sync::atomic::Ordering;
    let last_cycle = state
        .repair_state
        .last_cycle_at
        .read()
        .await
        .map(|t| t.to_rfc3339());
    let loop_running = state.repair_state.loop_running.load(Ordering::Relaxed);
    let records: Vec<_> = state
        .repair_state
        .records
        .read()
        .await
        .values()
        .cloned()
        .collect();
    Json(serde_json::json!({
        "loop_running": loop_running,
        "last_cycle_at": last_cycle,
        "repairs": records,
    }))
}

/// GET /api/v1/cost/status
///
/// Returns current spend and action-rate counters from the live CostGuard.
/// Useful for monitoring budget burn in real-time.
async fn cost_status_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    use std::sync::atomic::Ordering;

    let daily_cents = state.cost_guard.daily_spend_cents();
    let hourly_actions = state.cost_guard.hourly_action_count();
    let budget_exceeded = state.cost_guard.budget_exceeded.load(Ordering::Relaxed);
    let daily_limit = state.cost_guard.daily_limit_cents;
    let hourly_limit = state.cost_guard.hourly_action_limit;

    Json(serde_json::json!({
        "daily_spend_cents": daily_cents,
        "daily_limit_cents": daily_limit,
        "hourly_actions": hourly_actions,
        "hourly_action_limit": hourly_limit,
        "budget_exceeded": budget_exceeded,
    }))
}

// Canvas/A2UI Handlers

async fn canvas_ws_handler(
    ws: WebSocketUpgrade,
    Path(canvas_id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let canvas_id = crate::canvas::CanvasId(canvas_id);

    ws.on_upgrade(move |socket| handle_canvas_websocket(socket, canvas_id, state))
}

async fn handle_canvas_websocket(
    socket: axum::extract::ws::WebSocket,
    canvas_id: crate::canvas::CanvasId,
    state: Arc<GatewayState>,
) {
    use axum::extract::ws::Message;

    info!("Canvas WebSocket connected: {}", canvas_id.0);

    // Get or create canvas session
    let (event_tx, _event_rx) = mpsc::channel::<CanvasEvent>(100);
    let event_tx_client = event_tx.clone();

    let canvas_session = match state.canvas_manager.get_session(&canvas_id).await {
        Some(session) => session,
        None => state.canvas_manager.create_session(event_tx).await,
    };

    // Subscribe to updates
    let mut update_rx = canvas_session.update_tx.subscribe();

    // Split socket for send/receive
    let (mut sender, mut receiver) = socket.split();

    // Task to receive updates and send to client
    let update_task = tokio::spawn(async move {
        while let Ok(update) = update_rx.recv().await {
            let msg = Message::Text(serde_json::to_string(&update).unwrap_or_default());
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Task to receive client events and forward them into the canvas session
    let event_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<CanvasEvent>(&text) {
                    if event_tx_client.send(event).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = update_task => {}
        _ = event_task => {}
    }

    info!("Canvas WebSocket disconnected: {}", canvas_id.0);
}

async fn create_canvas_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let (event_tx, _) = mpsc::channel(100);
    let session = state.canvas_manager.create_session(event_tx).await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "canvas_id": session.id.0,
            "websocket_url": format!("/ws/canvas/{}", session.id.0),
        })),
    )
}

async fn get_canvas_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let canvas_id = crate::canvas::CanvasId(id.clone());

    match state.canvas_manager.get_session(&canvas_id).await {
        Some(session) => Json(serde_json::json!({
            "canvas_id": id,
            "status": "active",
            "session_id": session.id.0,
        }))
        .into_response(),
        None => {
            let error = serde_json::json!({
                "error": format!("Canvas '{}' not found", id),
                "canvas_id": id,
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

async fn delete_canvas_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let canvas_id = crate::canvas::CanvasId(id);
    state.canvas_manager.remove_session(&canvas_id).await;

    StatusCode::NO_CONTENT
}

// Provider Management Handlers

async fn list_providers_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let providers = state.model_router.list_providers().await;
    Json(serde_json::json!({
        "providers": providers,
        "count": providers.len(),
    }))
}

async fn get_provider_health_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.get_provider_health(&id).await {
        Some(health) => {
            let response = serde_json::json!({
                "provider": id,
                "health": health,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": format!("Provider '{}' not found", id),
                "provider": id,
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

async fn switch_model_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SwitchModelRequest>,
) -> impl IntoResponse {
    match state.model_router.switch_default_model(&body.model).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Switched to model '{}'", body.model),
                "current_model": body.model,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

async fn enable_provider_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.enable_provider(&id).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Provider '{}' enabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

async fn disable_provider_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.disable_provider(&id).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Provider '{}' disabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

async fn check_provider_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.check_provider_health(&id).await {
        Ok(healthy) => {
            let response = serde_json::json!({
                "provider": id,
                "healthy": healthy,
                "checked_at": chrono::Utc::now().to_rfc3339(),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

// Provider Usage Handlers

async fn provider_usage_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let snapshots = state.model_router.usage_tracker.all_snapshots().await;
    Json(serde_json::json!({
        "providers": snapshots,
        "count": snapshots.len(),
    }))
}

async fn provider_usage_by_id_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.usage_tracker.snapshot(&id).await {
        Some(snapshot) => (StatusCode::OK, Json(serde_json::json!(snapshot))).into_response(),
        None => {
            let error = serde_json::json!({
                "error": format!("No usage data found for provider '{}'", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

async fn get_fallback_chain_handler(
    Path(alias): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let chain = state.model_router.get_fallback_chain(&alias).await;
    Json(serde_json::json!({
        "alias": alias,
        "fallback_chain": chain,
    }))
}

#[derive(Debug, Deserialize)]
pub struct SetFallbackChainRequest {
    providers: Vec<String>,
}

async fn set_fallback_chain_handler(
    Path(alias): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SetFallbackChainRequest>,
) -> impl IntoResponse {
    match state
        .model_router
        .set_fallback_chain(&alias, body.providers)
        .await
    {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Fallback chain updated for '{}'", alias),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

// Auth Profile Handlers

async fn get_auth_profile_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.get_auth_profile_status(&id).await {
        Some(status) => (StatusCode::OK, Json(serde_json::json!(status))).into_response(),
        None => {
            let error = serde_json::json!({
                "error": format!("No auth profile found for provider '{}'", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

async fn rotate_auth_profile_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.model_router.rotate_auth_key(&id).await {
        Ok(_new_key) => {
            let response = serde_json::json!({
                "success": true,
                "provider": id,
                "message": format!("Auth key rotated for provider '{}'", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to rotate auth key: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

async fn list_auth_profiles_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let profiles = state.model_router.list_auth_profiles().await;
    Json(serde_json::json!({
        "profiles": profiles,
        "count": profiles.len(),
    }))
}

async fn list_models_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let entries = state.model_router.model_catalog.list().await;
    Json(serde_json::json!({
        "models": entries,
    }))
}

async fn get_default_model_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let default = state.model_router.get_default_model().await;
    Json(serde_json::json!({
        "default_model": default,
    }))
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

async fn memory_search_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<MemorySearchRequest>,
) -> impl IntoResponse {
    let vector_memory = state.vector_memory.read().await;
    match vector_memory.as_ref() {
        Some(vm) => {
            match vm
                .search_collection(&body.query, body.limit, &body.collection)
                .await
            {
                Ok(results) => {
                    let response = serde_json::json!({
                        "query": body.query,
                        "results": results,
                        "count": results.len(),
                    });
                    (StatusCode::OK, Json(response)).into_response()
                }
                Err(e) => {
                    let error = serde_json::json!({
                        "error": format!("Search failed: {}", e),
                    });
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
                }
            }
        }
        None => {
            let error = serde_json::json!({
                "error": "Vector memory service not enabled",
            });
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct MemoryAddRequest {
    content: String,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    collection: String,
}

async fn memory_add_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<MemoryAddRequest>,
) -> impl IntoResponse {
    let vector_memory = state.vector_memory.read().await;
    match vector_memory.as_ref() {
        Some(vm) => {
            match vm
                .add_to_collection(&body.content, body.metadata, &body.collection)
                .await
            {
                Ok(doc_id) => {
                    let response = serde_json::json!({
                        "document_id": doc_id,
                        "status": "added",
                    });
                    (StatusCode::CREATED, Json(response)).into_response()
                }
                Err(e) => {
                    let error = serde_json::json!({
                        "error": format!("Failed to add document: {}", e),
                    });
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
                }
            }
        }
        None => {
            let error = serde_json::json!({
                "error": "Vector memory service not enabled",
            });
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        }
    }
}

async fn list_memory_collections_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let vector_memory = state.vector_memory.read().await;
    match vector_memory.as_ref() {
        Some(vm) => {
            let collections = vm.list_collections();
            Json(serde_json::json!({
                "collections": collections,
                "count": collections.len(),
            }))
            .into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": "Vector memory service not enabled",
            });
            (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response()
        }
    }
}

// Plugin Management API Handlers

async fn list_plugins_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let plugins = state.plugin_manager.list_plugins().await;
    let plugin_list: Vec<_> = plugins
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id(),
                "name": p.name(),
                "enabled": p.enabled,
                "capabilities": p.manifest.capabilities,
            })
        })
        .collect();

    Json(serde_json::json!({
        "plugins": plugin_list,
        "count": plugin_list.len(),
    }))
}

async fn enable_plugin_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.plugin_manager.set_enabled(&id, true).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Plugin '{}' enabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to enable plugin: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

async fn disable_plugin_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.plugin_manager.set_enabled(&id, false).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Plugin '{}' disabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to disable plugin: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

async fn unload_plugin_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.plugin_manager.unload_plugin(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => {
            let error = serde_json::json!({
                "error": format!("Plugin '{}' not found", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to unload plugin: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

async fn reload_plugin_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.plugin_manager.reload_plugin(&id).await {
        Ok(reloaded_id) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Plugin '{}' reloaded", reloaded_id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to reload plugin: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

async fn reload_plugins_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    // Unload all currently loaded plugins, then re-initialize from disk.
    let plugins = state.plugin_manager.list_plugins().await;
    let ids: Vec<String> = plugins.iter().map(|p| p.id().to_string()).collect();
    let mut unloaded = 0usize;
    for id in &ids {
        match state.plugin_manager.unload_plugin(id).await {
            Ok(_) => unloaded += 1,
            Err(e) => warn!("Failed to unload plugin '{}' during reload: {}", id, e),
        }
    }
    match state.plugin_manager.initialize().await {
        Ok(loaded) => {
            let response = serde_json::json!({
                "success": true,
                "unloaded": unloaded,
                "loaded": loaded,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Reload failed: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

// Skills API Handlers

async fn list_skills_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let skills_manager = state.skills_manager.read().await;
    let skills = skills_manager.list_skills().await;

    let skill_list: Vec<_> = skills
        .iter()
        .map(|skill| {
            serde_json::json!({
                "id": skill.name.clone(),
                "name": skill.name.clone(),
                "description": skill.description.clone(),
                "enabled": skill.enabled,
                "is_eligible": skill.is_eligible,
                "triggers": skill.triggers.iter().map(|t| format!("{:?}", t.trigger_type)).collect::<Vec<_>>(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "skills": skill_list,
        "count": skill_list.len(),
    }))
}

async fn get_skill_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let skills_manager = state.skills_manager.read().await;
    match skills_manager.get_skill(&id).await {
        Some(skill) => {
            let response = serde_json::json!({
                "id": id,
                "name": skill.name,
                "description": skill.description,
                "enabled": skill.enabled,
                "is_eligible": skill.is_eligible,
                "triggers": skill.triggers.iter().map(|t| format!("{:?}", t.trigger_type)).collect::<Vec<_>>(),
                "eligibility_errors": skill.eligibility_errors,
            });
            Json(response).into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": format!("Skill '{}' not found", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

async fn enable_skill_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let mut skills_manager = state.skills_manager.write().await;
    match skills_manager.set_skill_enabled(&id, true).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Skill '{}' enabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to enable skill: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

async fn disable_skill_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let mut skills_manager = state.skills_manager.write().await;
    match skills_manager.set_skill_enabled(&id, false).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Skill '{}' disabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to disable skill: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RunSkillRequest {
    /// Input for the skill
    input: String,
    /// Additional context
    #[serde(default)]
    context: Option<serde_json::Value>,
}

async fn run_skill_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<RunSkillRequest>,
) -> impl IntoResponse {
    let skills_manager = state.skills_manager.read().await;

    // Activate skill with runtime requirement verification
    let skill = match skills_manager.activate_skill(&id).await {
        Ok(s) => s,
        Err(crate::error::MantaError::NotFound { .. }) => {
            let error = serde_json::json!({
                "error": format!("Skill '{}' not found", id),
            });
            return (StatusCode::NOT_FOUND, Json(error)).into_response();
        }
        Err(crate::error::MantaError::Validation(msg)) => {
            // Requirements not met at activation time
            let error = serde_json::json!({
                "error": "Skill requirements not met",
                "details": msg,
                "skill_id": id,
            });
            return (StatusCode::PRECONDITION_FAILED, Json(error)).into_response();
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to activate skill '{}': {}", id, e),
            });
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response();
        }
    };

    if !skill.enabled {
        let error = serde_json::json!({
            "error": format!("Skill '{}' is disabled", id),
        });
        return (StatusCode::BAD_REQUEST, Json(error)).into_response();
    }

    // Note: is_eligible is checked at load time, but activate_skill() also
    // verifies requirements at runtime. If we got here, requirements are met.

    // Build the message: skill system prompt + user input
    let full_message = if skill.prompt.is_empty() {
        body.input.clone()
    } else {
        format!("{}\n\nUser input: {}", skill.prompt, body.input)
    };

    // Capture trust level before dropping the lock (skill is owned so this is just being explicit)
    let skill_trust = skill.metadata.trust;

    // Drop read lock before acquiring agents lock
    drop(skills_manager);

    // Get the default agent's query channel to execute the skill
    let query_tx = {
        let agents = state.agents.read().await;
        agents.get("default").map(|h| h.query_tx.clone())
    };

    let query_tx = match query_tx {
        Some(tx) => tx,
        None => {
            let error = serde_json::json!({
                "error": "No default agent available to run skill",
            });
            return (StatusCode::SERVICE_UNAVAILABLE, Json(error)).into_response();
        }
    };

    // Execute via actor channel
    let session_id = format!("skill-{}-{}", id, uuid::Uuid::new_v4());
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if query_tx
        .send(AgentQuery::RunSkill {
            session_id: session_id.clone(),
            message: full_message,
            user_id: "skill-runner".to_string(),
            skill_trust,
            response_tx: resp_tx,
        })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "agent unavailable"})),
        )
            .into_response();
    }

    match resp_rx.await {
        Ok(Ok(outgoing)) => {
            let response = serde_json::json!({
                "skill_id": id,
                "session_id": session_id,
                "status": "completed",
                "result": outgoing.content,
                "usage": outgoing.usage,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Ok(Err(e)) => {
            let error = serde_json::json!({
                "error": format!("Skill execution failed: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "agent response channel closed"})),
        )
            .into_response(),
    }
}

// ACP (Agent Control Plane) API Handlers

async fn list_acp_sessions_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let subagents = state.acp.list_subagents().await;
    let sessions: Vec<_> = subagents
        .iter()
        .map(|s| {
            serde_json::json!({
                "subagent_id": s.id,
                "session_id": s.session_id.to_string(),
                "parent_id": s.parent_id,
                "mode": format!("{:?}", s.mode),
                "status": format!("{:?}", s.status),
                "thread_id": s.thread_id,
            })
        })
        .collect();

    Json(serde_json::json!({
        "sessions": sessions,
        "count": sessions.len(),
    }))
}

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

async fn acp_spawn_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SpawnSubagentRequest>,
) -> impl IntoResponse {
    use crate::acp::{AcpSessionId, SpawnMode, SubagentConfig, ThreadBinding};
    use crate::channels::IncomingMessage;
    use crate::security::runtime_audit::AuditEventType;
    use crate::security::RateLimitResult;
    use crate::security::UserId;

    // Rate limit: 10 spawns per minute per api-user
    let actor = "api-user";
    let rate_result = state
        .rate_limiter
        .check_with_cost(&UserId::new(format!("acp:spawn:{}", actor)), 1.0)
        .await;
    if !rate_result.is_allowed() {
        let retry = match rate_result {
            RateLimitResult::Denied { retry_after_secs } => retry_after_secs,
            _ => 60,
        };
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "Rate limit exceeded for ACP spawn",
                "retry_after": retry,
            })),
        )
            .into_response();
    }

    let session_id = AcpSessionId::new();
    let parent_id = "gateway-api".to_string();

    let mode = match body.mode.as_str() {
        "session" => SpawnMode::Session,
        _ => SpawnMode::Run,
    };

    let agent_type = if body.agent_type.is_empty() {
        "default".to_string()
    } else {
        body.agent_type.clone()
    };
    let config = SubagentConfig {
        agent_type: agent_type.clone(),
        mode,
        thread_binding: ThreadBinding::Auto,
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        tools: vec![],
        context: None,
        timeout_seconds: Some(300),
    };

    match state
        .acp
        .spawn_subagent(session_id.clone(), parent_id.clone(), config)
        .await
    {
        Ok(handle) => {
            let subagent_id = handle.id.clone();

            // Audit log
            state
                .audit_log
                .log(
                    AuditEventType::AcpSpawn,
                    actor,
                    &subagent_id,
                    true,
                    format!("Spawned subagent via API (mode: {:?})", handle.mode),
                    Some(serde_json::json!({
                        "session_id": session_id.to_string(),
                        "parent_id": parent_id,
                        "agent_type": agent_type,
                    })),
                )
                .await;

            // Send task to subagent
            let message =
                IncomingMessage::new(actor.to_string(), session_id.to_string(), body.task);

            match state.acp.send_message(&subagent_id, message).await {
                Ok(response) => {
                    let resp = serde_json::json!({
                        "subagent_id": subagent_id,
                        "session_id": session_id.to_string(),
                        "mode": format!("{:?}", handle.mode),
                        "response": response,
                    });
                    (StatusCode::CREATED, Json(resp)).into_response()
                }
                Err(e) => {
                    let _ = state.acp.shutdown_subagent(&subagent_id).await;
                    let error = serde_json::json!({
                        "error": format!("Subagent failed to process task: {}", e),
                    });
                    (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
                }
            }
        }
        Err(e) => {
            // Audit log failed spawn
            state
                .audit_log
                .log(
                    AuditEventType::AcpSpawn,
                    actor,
                    "",
                    false,
                    format!("Failed to spawn subagent: {}", e),
                    None,
                )
                .await;

            let error = serde_json::json!({
                "error": format!("Failed to spawn subagent: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

async fn terminate_acp_session_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    use crate::acp::AcpSessionId;
    use crate::security::runtime_audit::AuditEventType;

    let session_id = AcpSessionId(id.clone());
    match state.acp.terminate_session(&session_id).await {
        Ok(count) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpTerminate,
                    "api-user",
                    &id,
                    true,
                    format!("Terminated {} subagents in session {}", count, id),
                    Some(serde_json::json!({ "terminated_count": count })),
                )
                .await;
            let response = serde_json::json!({
                "terminated_count": count,
                "session_id": session_id.to_string(),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpTerminate,
                    "api-user",
                    &id,
                    false,
                    format!("Failed to terminate session: {}", e),
                    None,
                )
                .await;
            let error = serde_json::json!({
                "error": format!("Failed to terminate session: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AcpMessageRequest {
    message: String,
}

async fn acp_session_message_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<AcpMessageRequest>,
) -> impl IntoResponse {
    use crate::acp::AcpSessionId;
    use crate::channels::IncomingMessage;
    use crate::security::runtime_audit::AuditEventType;

    // Find a subagent in this session
    let session_id = AcpSessionId(id.clone());
    let subagents = state.acp.list_session_subagents(&session_id).await;

    if subagents.is_empty() {
        let error = serde_json::json!({
            "error": "No active subagents in session",
        });
        return (StatusCode::NOT_FOUND, Json(error)).into_response();
    }

    // Use the first active subagent
    let subagent = &subagents[0];
    let message =
        IncomingMessage::new("api-user".to_string(), session_id.to_string(), body.message);

    match state.acp.send_message(&subagent.id, message).await {
        Ok(response) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpMessage,
                    "api-user",
                    &id,
                    true,
                    format!("Message sent to subagent {} in session {}", subagent.id, id),
                    Some(serde_json::json!({
                        "subagent_id": subagent.id,
                        "session_id": id,
                    })),
                )
                .await;
            let resp = serde_json::json!({
                "subagent_id": subagent.id,
                "session_id": session_id.to_string(),
                "response": response,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpMessage,
                    "api-user",
                    &id,
                    false,
                    format!("Failed to send message: {}", e),
                    None,
                )
                .await;
            let error = serde_json::json!({
                "error": format!("Failed to send message: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

/// Get ACP session runtime status
async fn acp_session_status_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.acp.get_status(id.clone()).await {
        Some(status) => {
            let resp = serde_json::json!({
                "session_id": status.session_id,
                "runtime_state": format!("{}", status.runtime_state),
                "mode": format!("{:?}", status.mode),
                "current_iteration": status.current_iteration,
                "max_iterations": status.max_iterations,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": "Session not found",
                "session_id": id,
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

/// Pause an ACP session
async fn acp_session_pause_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    state.acp.pause(id.clone()).await;
    let resp = serde_json::json!({
        "session_id": id,
        "action": "pause",
        "status": "requested",
    });
    (StatusCode::OK, Json(resp)).into_response()
}

/// Resume a paused ACP session
async fn acp_session_resume_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    state.acp.resume(id.clone()).await;
    let resp = serde_json::json!({
        "session_id": id,
        "action": "resume",
        "status": "requested",
    });
    (StatusCode::OK, Json(resp)).into_response()
}

/// Single-step a paused ACP session
async fn acp_session_step_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    state.acp.step(id.clone()).await;
    let resp = serde_json::json!({
        "session_id": id,
        "action": "step",
        "status": "requested",
    });
    (StatusCode::OK, Json(resp)).into_response()
}

/// Cancel a running ACP session
async fn acp_session_cancel_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    state.acp.cancel(id.clone()).await;
    let resp = serde_json::json!({
        "session_id": id,
        "action": "cancel",
        "status": "requested",
    });
    (StatusCode::OK, Json(resp)).into_response()
}

/// Get subagent tree for an ACP session
async fn acp_session_tree_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let session_id = crate::acp::AcpSessionId(id.clone());
    let tree = state.acp.get_subagent_tree(&session_id).await;

    let resp = serde_json::json!({
        "session_id": id,
        "tree": tree,
    });
    (StatusCode::OK, Json(resp)).into_response()
}

#[derive(Debug, Deserialize)]
pub struct AcpExecuteRequest {
    message: String,
    user_id: String,
    agent_id: Option<String>,
}

/// Execute a message in ACP session mode (persistent context)
async fn acp_execute_session_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<AcpExecuteRequest>,
) -> impl IntoResponse {
    let agent_id = body.agent_id.unwrap_or_else(|| "default".to_string());
    let agents = state.agents.read().await;
    let agent_handle = match agents.get(&agent_id) {
        Some(h) => h.clone(),
        None => {
            let error = serde_json::json!({
                "error": format!("Agent '{}' not found", agent_id),
            });
            return (StatusCode::NOT_FOUND, Json(error)).into_response();
        }
    };
    drop(agents);

    let session_id = uuid::Uuid::new_v4().to_string();
    let incoming = crate::channels::IncomingMessage::new(
        body.user_id.clone(),
        session_id.clone(),
        body.message,
    );

    match state
        .acp
        .execute_session(agent_handle.agent, incoming)
        .await
    {
        Ok(outgoing) => {
            let resp = serde_json::json!({
                "session_id": session_id,
                "mode": "session",
                "response": outgoing.content,
                "usage": outgoing.usage,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Execution failed: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

/// Execute a message in ACP run mode (one-shot, no persistence)
async fn acp_execute_run_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<AcpExecuteRequest>,
) -> impl IntoResponse {
    let agent_id = body.agent_id.unwrap_or_else(|| "default".to_string());
    let agents = state.agents.read().await;
    let agent_handle = match agents.get(&agent_id) {
        Some(h) => h.clone(),
        None => {
            let error = serde_json::json!({
                "error": format!("Agent '{}' not found", agent_id),
            });
            return (StatusCode::NOT_FOUND, Json(error)).into_response();
        }
    };
    drop(agents);

    let session_id = uuid::Uuid::new_v4().to_string();
    let incoming = crate::channels::IncomingMessage::new(
        body.user_id.clone(),
        session_id.clone(),
        body.message,
    );

    match state.acp.execute_run(agent_handle.agent, incoming).await {
        Ok(outgoing) => {
            let resp = serde_json::json!({
                "session_id": session_id,
                "mode": "run",
                "response": outgoing.content,
                "usage": outgoing.usage,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Execution failed: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}

/// Handler to spawn a discovered agent from the registry
async fn spawn_discovered_agent_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    info!("API request to spawn discovered agent: {}", id);

    // Check if agent is already running
    {
        let agents = state.agents.read().await;
        if agents.contains_key(&id) {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": format!("Agent '{}' is already running", id),
                    "agent_id": id,
                })),
            )
                .into_response();
        }
    }

    // Check if agent is in registry
    {
        let registry = state.agent_registry.read().await;
        if !registry.has(&id) {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Agent '{}' not found in registry", id),
                    "available_agents": registry.list(),
                })),
            )
                .into_response();
        }
    }

    // Spawn the agent
    // Note: This requires access to the Gateway, so we need to spawn manually
    let personality = {
        let registry = state.agent_registry.read().await;
        registry.get(&id).cloned()
    };

    if let Some(personality) = personality {
        let config = personality.to_agent_config();

        // Create provider from model router
        let provider = match state.model_router.create_default_provider().await {
            Ok(p) => p,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("Failed to create provider: {}", e),
                    })),
                )
                    .into_response();
            }
        };

        let tools = state.tool_registry.clone();
        let model = state.config.read().await.model.clone();
        let memory_manager = state.memory_manager.read().await.clone();
        let (tx, mut rx) = mpsc::channel(100);

        let agent = if let Some(mm) = memory_manager {
            Arc::new(
                Agent::new(config.clone(), provider, tools)
                    .with_model(model)
                    .with_memory_manager(mm)
                    .with_transcript_store(Arc::clone(&state.transcript_store))
                    .with_artifact_store(Arc::clone(&state.artifact_store))
                    .with_disk_budget(Arc::clone(&state.disk_budget))
                    .with_session_file_manager(Arc::clone(&state.session_file_manager)),
            )
        } else {
            Arc::new(
                Agent::new(config.clone(), provider, tools)
                    .with_model(model)
                    .with_transcript_store(Arc::clone(&state.transcript_store))
                    .with_artifact_store(Arc::clone(&state.artifact_store))
                    .with_disk_budget(Arc::clone(&state.disk_budget))
                    .with_session_file_manager(Arc::clone(&state.session_file_manager)),
            )
        };

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
        let state_clone = state.clone();
        let agent_id_clone = id.clone();
        tokio::spawn(async move {
            info!("Agent {} processing loop started", agent_id_clone);
            loop {
                tokio::select! {
                    cmd = rx.recv() => {
                    let cmd = match cmd { Some(c) => c, None => break };
                    match cmd {
                        AgentCommand::Shutdown => {
                            info!("Agent {} shutting down", agent_id_clone);
                            let _ = state_clone.event_tx.send(GatewayEvent::AgentStatus {
                                agent_id: agent_id_clone.clone(),
                                status: AgentStatus::Shutdown,
                            });
                            break;
                        }
                        AgentCommand::ProcessMessage {
                            session_id,
                            message,
                            user_id,
                            channel,
                            model_override,
                        } => {
                            let incoming_msg = crate::channels::IncomingMessage::new(
                                user_id.clone(),
                                session_id.clone(),
                                message.clone(),
                            );

                            agent.set_model_override(model_override).await;
                            let result = agent.process_message(incoming_msg).await;
                            agent.set_model_override(None).await;

                            match result {
                                Ok(outgoing) => {
                                    // Route response back to channel
                                    let _ = state_clone.event_tx.send(GatewayEvent::AgentResponse {
                                        session_id: session_id.clone(),
                                        agent_id: agent_id_clone.clone(),
                                        content: outgoing.content,
                                        channel: channel.clone(),
                                        conversation_id: session_id.clone(),
                                        usage: outgoing.usage,
                                    });
                                }
                                Err(e) => {
                                    error!("Agent {} failed to process message: {}", agent_id_clone, e);
                                }
                            }
                        }
                        _ => {
                            info!("Agent {} received command: {:?}", agent_id_clone, cmd);
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
                                let result = agent.process_message_with_progress(incoming, no_op).await;
                                agent.set_skill_trust(crate::tools::SkillTrust::Trusted);
                                let _ = response_tx.send(result);
                            }
                        }
                    }
                }
            } // end tokio::select! and loop
            info!("Agent {} processing loop ended", agent_id_clone);
        });

        info!("✅ Spawned discovered agent '{}' from registry", id);
        (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "agent_id": id,
                "status": "spawned",
                "source": "registry",
            })),
        )
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("Agent '{}' not found in registry", id),
            })),
        )
            .into_response()
    }
}

/// Handler to spawn all discovered agents
async fn spawn_all_discovered_agents_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    info!("API request to spawn all discovered agents");

    let agent_ids: Vec<String> = {
        let registry = state.agent_registry.read().await;
        registry.list()
    };

    let mut spawned = 0;
    let mut already_running = 0;
    let mut failed = 0;

    for agent_id in agent_ids {
        // Check if already running
        {
            let agents = state.agents.read().await;
            if agents.contains_key(&agent_id) {
                already_running += 1;
                continue;
            }
        }

        // Spawn the agent
        let personality = {
            let registry = state.agent_registry.read().await;
            registry.get(&agent_id).cloned()
        };

        if let Some(personality) = personality {
            let config = personality.to_agent_config();

            if let Ok(provider) = state.model_router.create_default_provider().await {
                let tools = state.tool_registry.clone();
                let model = state.config.read().await.model.clone();
                let memory_manager = state.memory_manager.read().await.clone();
                let (tx, mut rx) = mpsc::channel(100);

                let agent = if let Some(mm) = memory_manager {
                    Arc::new(
                        Agent::new(config.clone(), provider, tools)
                            .with_model(model)
                            .with_memory_manager(mm)
                            .with_transcript_store(Arc::clone(&state.transcript_store))
                            .with_artifact_store(Arc::clone(&state.artifact_store))
                            .with_disk_budget(Arc::clone(&state.disk_budget))
                            .with_session_file_manager(Arc::clone(&state.session_file_manager)),
                    )
                } else {
                    Arc::new(
                        Agent::new(config.clone(), provider, tools)
                            .with_model(model)
                            .with_transcript_store(Arc::clone(&state.transcript_store))
                            .with_artifact_store(Arc::clone(&state.artifact_store))
                            .with_disk_budget(Arc::clone(&state.disk_budget))
                            .with_session_file_manager(Arc::clone(&state.session_file_manager)),
                    )
                };

                let (query_tx, mut query_rx) = mpsc::channel::<AgentQuery>(32);

                let handle = AgentHandle {
                    id: agent_id.clone(),
                    config: config.clone(),
                    tx: tx.clone(),
                    query_tx: query_tx.clone(),
                    busy: false,
                    agent: agent.clone(),
                };

                {
                    let mut agents = state.agents.write().await;
                    agents.insert(agent_id.clone(), handle);
                }

                // Start processing loop
                let state_clone = state.clone();
                let agent_id_clone = agent_id.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            cmd = rx.recv() => {
                                let cmd = match cmd { Some(c) => c, None => break };
                                if let AgentCommand::Shutdown = cmd { break; }
                            }
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
                                        let result = agent.process_message_with_progress(incoming, no_op).await;
                                        agent.set_skill_trust(crate::tools::SkillTrust::Trusted);
                                        let _ = response_tx.send(result);
                                    }
                                }
                            }
                        }
                    }
                    let _ = state_clone.event_tx.send(GatewayEvent::AgentStatus {
                        agent_id: agent_id_clone,
                        status: AgentStatus::Shutdown,
                    });
                });

                spawned += 1;
            } else {
                failed += 1;
            }
        } else {
            failed += 1;
        }
    }

    info!(
        "Spawned {} agents, {} already running, {} failed",
        spawned, already_running, failed
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "spawned": spawned,
            "already_running": already_running,
            "failed": failed,
        })),
    )
        .into_response()
}

/// Handler to list discovered agents in registry
async fn list_discovered_agents_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let registry = state.agent_registry.read().await;
    let agents = state.agents.read().await;

    let list: Vec<_> = registry
        .iter()
        .map(|p| {
            let is_running = agents.contains_key(&p.id);
            serde_json::json!({
                "id": p.id,
                "name": p.display_name(),
                "running": is_running,
                "valid": p.is_valid,
            })
        })
        .collect();

    Json(list)
}

// ─────────────────────────────────────────────
// MCP REST API handlers (9.5)
// ─────────────────────────────────────────────

/// List connected MCP servers
async fn list_mcp_servers_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let servers = state.mcp_manager.list_servers().await;
    Json(serde_json::json!({
        "servers": servers,
        "count": servers.len(),
    }))
}

/// Request body for connecting an MCP server
#[derive(Debug, Deserialize)]
struct McpConnectRequest {
    #[serde(default)]
    transport: String,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    url: Option<String>,
    #[serde(default = "mcp_default_timeout")]
    timeout_secs: u64,
}

fn mcp_default_timeout() -> u64 {
    30
}

/// Connect to an MCP server
async fn connect_mcp_server_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
    Json(body): Json<McpConnectRequest>,
) -> impl IntoResponse {
    use crate::tools::mcp::{McpServerConfig, McpTransport};

    let transport = match body.transport.as_str() {
        "sse" => McpTransport::Sse,
        "streamable_http" => McpTransport::StreamableHttp,
        _ => McpTransport::Stdio,
    };

    let config = McpServerConfig {
        transport,
        command: body.command,
        args: body.args,
        url: body.url,
        timeout_secs: body.timeout_secs,
        ..Default::default()
    };

    match state.mcp_manager.connect(&server_id, config).await {
        Ok(tools) => (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({
                "server_id": server_id,
                "tool_count": tools.len(),
                "tools": tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
            })),
        ),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// Disconnect from an MCP server
async fn disconnect_mcp_server_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
) -> impl IntoResponse {
    match state.mcp_manager.disconnect(&server_id).await {
        Ok(()) => {
            // Remove all `mcp__{server_id}__*` tools from the registry so
            // they are no longer offered to agents.
            let prefix = format!("mcp__{server_id}__");
            state.tool_registry.deregister_prefix(&prefix);

            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({ "disconnected": server_id })),
            )
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// List tools from an MCP server
async fn list_mcp_tools_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
) -> impl IntoResponse {
    match state.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            let tools = client.get_tools().to_vec();
            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({
                    "server_id": server_id,
                    "tools": tools,
                    "count": tools.len(),
                })),
            )
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("MCP server '{}' not found", server_id) })),
        ),
    }
}

/// Call an MCP tool
async fn call_mcp_tool_handler(
    State(state): State<Arc<GatewayState>>,
    Path((server_id, tool_name)): Path<(String, String)>,
    Json(args): Json<serde_json::Value>,
) -> impl IntoResponse {
    match state.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client.call_tool(&tool_name, args).await {
                Ok(result) => {
                    (axum::http::StatusCode::OK, Json(serde_json::json!({ "result": result })))
                }
                Err(e) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                ),
            }
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("MCP server '{}' not found", server_id) })),
        ),
    }
}

/// List resources from an MCP server
async fn list_mcp_resources_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
) -> impl IntoResponse {
    match state.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client.list_resources().await {
                Ok(resources) => (
                    axum::http::StatusCode::OK,
                    Json(serde_json::json!({
                        "server_id": server_id,
                        "resources": resources,
                        "count": resources.len(),
                    })),
                ),
                Err(e) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                ),
            }
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("MCP server '{}' not found", server_id) })),
        ),
    }
}

/// Request body for reading a resource
#[derive(Debug, Deserialize)]
struct McpReadResourceRequest {
    uri: String,
}

/// Read a resource from an MCP server
async fn read_mcp_resource_handler(
    State(state): State<Arc<GatewayState>>,
    Path(server_id): Path<String>,
    Json(body): Json<McpReadResourceRequest>,
) -> impl IntoResponse {
    match state.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client.read_resource(&body.uri).await {
                Ok(contents) => (
                    axum::http::StatusCode::OK,
                    Json(serde_json::json!({
                        "uri": body.uri,
                        "contents": contents,
                    })),
                ),
                Err(e) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                ),
            }
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("MCP server '{}' not found", server_id) })),
        ),
    }
}

// ─────────────────────────────────────────────
// 9.9 – Manta as an MCP server
// ─────────────────────────────────────────────

/// Expose Manta's tool registry as an MCP server via the Streamable-HTTP transport.
///
/// Handles JSON-RPC 2.0 requests sent to `POST /mcp`.  Supported methods:
/// - `initialize` – returns server capabilities
/// - `tools/list` – lists all registered tools
/// - `tools/call` – calls a registered tool
///
/// The response content-type is `text/event-stream` (SSE) when the caller
/// sends `Accept: text/event-stream`, or plain `application/json` otherwise.
async fn manta_as_mcp_server_handler(
    State(state): State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    use axum::http::header;

    // Parse the incoming JSON-RPC request.
    let request: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return json_rpc_error_response(None, -32700, &format!("Parse error: {}", e));
        }
    };

    let id = request.get("id").cloned();
    let method = match request["method"].as_str() {
        Some(m) => m.to_string(),
        None => {
            return json_rpc_error_response(id.as_ref(), -32600, "Invalid request: missing method");
        }
    };

    let result: serde_json::Value = match method.as_str() {
        "initialize" => {
            let tools = state.tool_registry.get_definitions();
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "manta",
                    "version": crate::VERSION,
                },
                "capabilities": {
                    "tools": { "count": tools.len() }
                }
            })
        }

        "tools/list" => {
            let defs = state.tool_registry.get_definitions();
            let tools: Vec<serde_json::Value> = defs
                .iter()
                .map(|d| {
                    serde_json::json!({
                        "name": d.name,
                        "description": d.description,
                        "inputSchema": d.parameters,
                    })
                })
                .collect();
            serde_json::json!({ "tools": tools })
        }

        "tools/call" => {
            let params = &request["params"];
            let tool_name = match params["name"].as_str() {
                Some(n) => n.to_string(),
                None => {
                    return json_rpc_error_response(id.as_ref(), -32602, "Missing tool name");
                }
            };
            let args = params["arguments"].clone();

            let context = crate::tools::ToolContext::default();
            match state
                .tool_registry
                .execute(&tool_name, args, &context)
                .await
            {
                Some(Ok(exec_result)) => {
                    serde_json::json!({
                        "content": [{
                            "type": "text",
                            "text": exec_result.output,
                        }]
                    })
                }
                Some(Err(e)) => {
                    return json_rpc_error_response(
                        id.as_ref(),
                        -32603,
                        &format!("Tool error: {}", e),
                    );
                }
                None => {
                    return json_rpc_error_response(
                        id.as_ref(),
                        -32601,
                        &format!("Tool not found: {}", tool_name),
                    );
                }
            }
        }

        _ => {
            return json_rpc_error_response(
                id.as_ref(),
                -32601,
                &format!("Method not found: {}", method),
            );
        }
    };

    let response_json = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    });

    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if accept.contains("text/event-stream") {
        // Respond as SSE
        let sse_body = format!("data: {}\n\n", response_json);
        axum::response::Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(axum::body::Body::from(sse_body))
            .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
    } else {
        axum::response::Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(response_json.to_string()))
            .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
    }
}

/// Helper: build a JSON-RPC error response as an Axum Response.
fn json_rpc_error_response(
    id: Option<&serde_json::Value>,
    code: i32,
    message: &str,
) -> axum::response::Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    });
    axum::response::Response::builder()
        .status(200)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
}

// ── OpenAI-compatible API ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(default)]
    stream: bool,
}

/// Query parameters for model override.
#[derive(Debug, Deserialize)]
struct ModelOverrideQuery {
    #[serde(rename = "model")]
    model: Option<String>,
}

#[derive(Debug, Serialize)]
struct OpenAiChatResponse {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<OpenAiChoice>,
    usage: OpenAiUsage,
}

#[derive(Debug, Serialize)]
struct OpenAiChoice {
    index: u32,
    message: OpenAiResponseMessage,
    finish_reason: String,
}

#[derive(Debug, Serialize)]
struct OpenAiResponseMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

/// `POST /v1/chat/completions`
///
/// OpenAI-compatible chat completions endpoint. Routes the last user message
/// through the default Manta agent and returns the result in OpenAI wire
/// format. Supports both streaming (`stream: true` → SSE) and non-streaming.
#[allow(unused_assignments)]
async fn openai_chat_completions_handler(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<ModelOverrideQuery>,
    headers: axum::http::HeaderMap,
    Json(mut req): Json<OpenAiChatRequest>,
) -> axum::response::Response {
    use axum::response::sse::{Event as SseEvt, KeepAlive, Sse};

    // Request-level model override: header X-Model takes precedence,
    // then query param ?model=..., then JSON body model field.
    if let Some(header_model) = headers.get("x-model").and_then(|v| v.to_str().ok()) {
        req.model = header_model.to_string();
    } else if let Some(query_model) = query.model {
        req.model = query_model;
    }

    // Extract the last user message.
    let user_message = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_default();

    if user_message.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "message": "No user message provided",
                    "type": "invalid_request_error"
                }
            })),
        )
            .into_response();
    }

    // Grab the default agent handle.
    let handle = {
        let agents = state.agents.read().await;
        match agents.get("default").cloned() {
            Some(h) => h,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": {"message": "No agent available", "type": "server_error"}
                    })),
                )
                    .into_response();
            }
        }
    };

    // Subscribe to events before sending the command to avoid a race.
    let mut event_rx = state.event_tx.subscribe();
    let session_id = uuid::Uuid::new_v4().to_string();

    let cmd = AgentCommand::ProcessMessage {
        session_id: session_id.clone(),
        message: user_message,
        user_id: "openai_api".to_string(),
        channel: "api".to_string(),
        model_override: Some(req.model.clone()),
    };

    if let Err(e) = handle.tx.send(cmd).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {"message": format!("Agent error: {}", e), "type": "server_error"}
            })),
        )
            .into_response();
    }

    if req.stream {
        // ── Streaming SSE response ──────────────────────────────────────────
        let model = req.model.clone();
        let (tx, rx) = mpsc::channel::<Result<SseEvt, std::convert::Infallible>>(64);

        tokio::spawn(async move {
            let completion_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
            let created = chrono::Utc::now().timestamp();
            let timeout_dur = tokio::time::Duration::from_secs(120);
            let start = tokio::time::Instant::now();

            // Wait for the full agent response.
            let response_content = loop {
                if start.elapsed() > timeout_dur {
                    break String::new();
                }
                match tokio::time::timeout(tokio::time::Duration::from_millis(100), event_rx.recv())
                    .await
                {
                    Ok(Ok(GatewayEvent::AgentResponse { session_id: sid, content, .. })) => {
                        if sid == session_id {
                            break content;
                        }
                    }
                    Ok(Err(_)) | Err(_) => {}
                    _ => {}
                }
            };

            // Stream the response word-by-word.
            for word in response_content.split_inclusive(|c: char| c.is_whitespace()) {
                let chunk = serde_json::json!({
                    "id": completion_id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": model,
                    "choices": [{"index": 0, "delta": {"content": word}, "finish_reason": null}]
                });
                let _ = tx.send(Ok(SseEvt::default().data(chunk.to_string()))).await;
            }

            // Final chunk with finish_reason = "stop".
            let final_chunk = serde_json::json!({
                "id": completion_id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            });
            let _ = tx
                .send(Ok(SseEvt::default().data(final_chunk.to_string())))
                .await;
            let _ = tx
                .send(Ok(SseEvt::default().data("[DONE]".to_string())))
                .await;
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response()
    } else {
        // ── Non-streaming JSON response ─────────────────────────────────────
        let timeout_dur = tokio::time::Duration::from_secs(120);
        let start = tokio::time::Instant::now();
        let mut response_content: Option<String> = None;
        let mut prompt_tokens = 0u32;
        let mut completion_tokens = 0u32;
        let mut total_tokens = 0u32;

        loop {
            if start.elapsed() > timeout_dur {
                return (
                    StatusCode::REQUEST_TIMEOUT,
                    Json(serde_json::json!({
                        "error": {"message": "Request timed out", "type": "server_error"}
                    })),
                )
                    .into_response();
            }

            match tokio::time::timeout(tokio::time::Duration::from_millis(100), event_rx.recv())
                .await
            {
                Ok(Ok(GatewayEvent::AgentResponse {
                    session_id: sid,
                    content,
                    usage,
                    ..
                })) => {
                    if sid == session_id {
                        response_content = Some(content);
                        if let Some(ref u) = usage {
                            prompt_tokens = u.prompt_tokens;
                            completion_tokens = u.completion_tokens;
                            total_tokens = u.total_tokens;
                        }
                        break;
                    }
                }
                Ok(Err(_)) | Err(_) => {}
                _ => {}
            }
        }

        let resp = OpenAiChatResponse {
            id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".to_string(),
            created: chrono::Utc::now().timestamp(),
            model: req.model.clone(),
            choices: vec![OpenAiChoice {
                index: 0,
                message: OpenAiResponseMessage {
                    role: "assistant".to_string(),
                    content: response_content.unwrap_or_default(),
                },
                finish_reason: "stop".to_string(),
            }],
            usage: OpenAiUsage {
                prompt_tokens,
                completion_tokens,
                total_tokens,
            },
        };

        Json(resp).into_response()
    }
}

/// `GET /v1/models`
///
/// Returns available model aliases in OpenAI wire format.
async fn openai_list_models_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let entries = state.model_router.model_catalog.list().await;
    let data: Vec<_> = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.id.clone(),
                "object": "model",
                "created": 0,
                "owned_by": entry.provider.clone(),
            })
        })
        .collect();

    Json(serde_json::json!({ "object": "list", "data": data }))
}

// ── Runtime settings CRUD ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SetSettingRequest {
    key: String,
    value: serde_json::Value,
}

/// `GET /api/settings` — list all runtime key/value settings.
async fn list_settings_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let settings = state.runtime_settings.read().await.clone();
    Json(settings)
}

/// `POST /api/settings` — upsert a runtime setting.
async fn set_setting_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SetSettingRequest>,
) -> impl IntoResponse {
    let mut settings = state.runtime_settings.write().await;
    settings.insert(req.key.clone(), req.value.clone());
    Json(serde_json::json!({ "ok": true, "key": req.key }))
}

/// `GET /api/settings/:key` — read one setting by key.
async fn get_setting_handler(
    State(state): State<Arc<GatewayState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let settings = state.runtime_settings.read().await;
    match settings.get(&key) {
        Some(val) => Json(serde_json::json!({ "key": key, "value": val })).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Setting '{}' not found", key) })),
        )
            .into_response(),
    }
}

/// `DELETE /api/settings/:key` — remove one setting.
async fn delete_setting_handler(
    State(state): State<Arc<GatewayState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let mut settings = state.runtime_settings.write().await;
    if settings.remove(&key).is_some() {
        Json(serde_json::json!({ "ok": true, "key": key })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Setting '{}' not found", key) })),
        )
            .into_response()
    }
}

// ── Tool approval management (human-in-the-loop) ──────────────────────────────

/// `GET /api/v1/approvals` — list all pending approval requests.
async fn list_approvals_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let approvals = state
        .approval_queue
        .list_pending(ApprovalFilter::default())
        .await;
    Json(serde_json::json!({ "approvals": approvals, "count": approvals.len() }))
}

/// `GET /api/v1/approvals/:id` — get a specific pending approval.
async fn get_approval_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.approval_queue.get(&id).await {
        Some(approval) => Json(approval).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Approval '{}' not found", id) })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/approvals/:id/approve` — approve a pending tool call.
async fn approve_tool_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    if state
        .approval_queue
        .resolve(&id, ApprovalDecision::Approve)
        .await
    {
        Json(serde_json::json!({ "id": id, "status": "approved" })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Approval '{}' not found", id) })),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
struct DenyApprovalRequest {
    reason: Option<String>,
}

/// `POST /api/v1/approvals/:id/deny` — deny a pending tool call.
async fn deny_tool_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    body: Option<Json<DenyApprovalRequest>>,
) -> impl IntoResponse {
    let reason = body
        .and_then(|b| b.reason.clone())
        .unwrap_or_else(|| "Denied by operator".to_string());

    if state
        .approval_queue
        .resolve(&id, ApprovalDecision::Deny { reason: reason.clone() })
        .await
    {
        Json(serde_json::json!({ "id": id, "status": "denied", "reason": reason })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Approval '{}' not found", id) })),
        )
            .into_response()
    }
}

// ── Cron job management ───────────────────────────────────────────────────────

/// `GET /api/v1/cron` — list all scheduled jobs.
async fn list_cron_jobs_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let guard = state.cron_scheduler.read().await;
    match guard.as_ref() {
        Some(scheduler) => {
            let jobs = scheduler.lock().await.list_jobs().await;
            Json(serde_json::json!({ "jobs": jobs, "count": jobs.len() })).into_response()
        }
        None => Json(serde_json::json!({ "jobs": [], "count": 0 })).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct AddCronJobRequest {
    name: String,
    schedule: String,
    command: String,
}

/// `POST /api/v1/cron` — create a new cron job.
async fn add_cron_job_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AddCronJobRequest>,
) -> impl IntoResponse {
    use crate::cron::cron::{CronJob, ExecutionTarget, Schedule as CronSchedule};
    use std::str::FromStr;

    let schedule = match cron::Schedule::from_str(&req.schedule) {
        Ok(_) => CronSchedule::Cron {
            expression: req.schedule.clone(),
            timezone: None,
            stagger_ms: None,
        },
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid cron expression: {}", e) })),
            )
                .into_response();
        }
    };

    let job_id = uuid::Uuid::new_v4().to_string();
    let job = CronJob::new(
        job_id.clone(),
        req.name.clone(),
        schedule,
        ExecutionTarget::shell(req.command),
    );

    let guard = state.cron_scheduler.read().await;
    match guard.as_ref() {
        Some(scheduler) => match scheduler.lock().await.add_job(job).await {
            Ok(()) => Json(serde_json::json!({
                "success": true,
                "id": job_id,
                "name": req.name,
            }))
            .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to add job: {}", e) })),
            )
                .into_response(),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/cron/:id` — remove a cron job.
async fn remove_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let guard = state.cron_scheduler.read().await;
    match guard.as_ref() {
        Some(scheduler) => match scheduler.lock().await.remove_job(&id).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id })).into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/cron/:id/enable` — enable a cron job.
async fn enable_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let guard = state.cron_scheduler.read().await;
    match guard.as_ref() {
        Some(scheduler) => match scheduler.lock().await.set_job_enabled(&id, true).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id, "enabled": true }))
                .into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/cron/:id/disable` — disable a cron job.
async fn disable_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let guard = state.cron_scheduler.read().await;
    match guard.as_ref() {
        Some(scheduler) => match scheduler.lock().await.set_job_enabled(&id, false).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id, "enabled": false }))
                .into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/cron/:id/run` — trigger a cron job immediately.
async fn trigger_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let guard = state.cron_scheduler.read().await;
    match guard.as_ref() {
        Some(scheduler) => match scheduler.lock().await.trigger_job(&id).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id, "triggered": true }))
                .into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/cron/:id/logs` — return job state / last-run info.
async fn cron_job_logs_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let guard = state.cron_scheduler.read().await;
    match guard.as_ref() {
        Some(scheduler) => match scheduler.lock().await.get_job(&id).await {
            Some(job) => Json(serde_json::json!({
                "id": job.id,
                "name": job.name,
                "enabled": job.enabled,
                "run_count": job.state.run_count,
                "last_run_at": job.state.last_run_at,
                "next_run_at": job.state.next_run_at,
                "last_error": job.state.last_error,
                "consecutive_errors": job.state.consecutive_errors,
            }))
            .into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("Job '{}' not found", id) })),
            )
                .into_response(),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

// ── Entity management ─────────────────────────────────────────────────────────

/// `GET /api/v1/entities` — list all entities.
async fn list_entities_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let storage = state.storage.read().await;
    match storage.list().await {
        Ok(entities) => Json(serde_json::json!({
            "entities": entities,
            "count": entities.len(),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct CreateEntityRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    status: Option<String>,
}

/// `POST /api/v1/entities` — create a new entity.
async fn create_entity_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<CreateEntityRequest>,
) -> impl IntoResponse {
    use crate::core::models::{Entity, Status};

    let mut entity = Entity::new(req.name);
    if let Some(desc) = req.description {
        entity = entity.with_description(desc);
    }
    if let Some(tags) = req.tags {
        entity = entity.with_tags(tags);
    }
    if let Some(status_str) = req.status {
        if let Ok(s) = status_str.parse::<Status>() {
            entity = entity.with_status(s);
        }
    }

    let storage = state.storage.read().await;
    match storage.create(&entity).await {
        Ok(()) => (StatusCode::CREATED, Json(entity)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/entities/:id` — get a single entity.
async fn get_entity_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    use crate::core::models::Id;

    let entity_id = match Id::parse(&id) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid ID: {}", e) })),
            )
                .into_response();
        }
    };

    let storage = state.storage.read().await;
    match storage.get(entity_id).await {
        Ok(entity) => Json(entity).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateEntityRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

/// `PUT /api/v1/entities/:id` — update an entity.
async fn update_entity_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<UpdateEntityRequest>,
) -> impl IntoResponse {
    use crate::core::models::{Id, Status};

    let entity_id = match Id::parse(&id) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid ID: {}", e) })),
            )
                .into_response();
        }
    };

    let storage = state.storage.read().await;
    let mut entity = match storage.get(entity_id).await {
        Ok(e) => e,
        Err(e) => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                .into_response();
        }
    };

    if let Some(name) = req.name {
        entity.set_name(name);
    }
    if let Some(desc) = req.description {
        entity.description = Some(desc);
    }
    if let Some(tags) = req.tags {
        entity.tags = Some(tags);
    }
    if let Some(status_str) = req.status {
        if let Ok(s) = status_str.parse::<Status>() {
            entity.status = s;
        }
    }
    entity.metadata.touch();

    match storage.update(&entity).await {
        Ok(()) => Json(entity).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

/// `DELETE /api/v1/entities/:id` — delete an entity.
async fn delete_entity_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    use crate::core::models::Id;

    let entity_id = match Id::parse(&id) {
        Ok(i) => i,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid ID: {}", e) })),
            )
                .into_response();
        }
    };

    let storage = state.storage.read().await;
    match storage.delete(entity_id).await {
        Ok(()) => Json(serde_json::json!({ "success": true, "id": id })).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SearchEntitiesRequest {
    query: String,
    #[serde(default)]
    entity_type: Option<String>,
}

/// `POST /api/v1/entities/search` — search entities by name.
async fn search_entities_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SearchEntitiesRequest>,
) -> impl IntoResponse {
    let storage = state.storage.read().await;
    match storage.list().await {
        Ok(entities) => {
            let query_lower = req.query.to_lowercase();
            let results: Vec<_> = entities
                .into_iter()
                .filter(|e| {
                    e.name.to_lowercase().contains(&query_lower)
                        || e.description
                            .as_deref()
                            .map(|d| d.to_lowercase().contains(&query_lower))
                            .unwrap_or(false)
                })
                .collect();
            Json(serde_json::json!({ "results": results, "count": results.len() })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/entities/export` — export all entities as JSON.
async fn export_entities_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let storage = state.storage.read().await;
    match storage.list().await {
        Ok(entities) => Json(serde_json::json!({ "entities": entities, "count": entities.len() }))
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct ImportEntitiesRequest {
    entities: Vec<serde_json::Value>,
}

/// `POST /api/v1/entities/import` — bulk import entities.
async fn import_entities_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ImportEntitiesRequest>,
) -> impl IntoResponse {
    use crate::core::models::Entity;

    let storage = state.storage.read().await;
    let mut imported = 0usize;
    let mut errors = Vec::<String>::new();

    for val in req.entities {
        match serde_json::from_value::<Entity>(val) {
            Ok(entity) => match storage.create(&entity).await {
                Ok(()) => imported += 1,
                Err(e) => errors.push(format!("{}: {}", entity.name, e)),
            },
            Err(e) => errors.push(format!("Parse error: {}", e)),
        }
    }

    Json(serde_json::json!({
        "imported": imported,
        "errors": errors,
    }))
    .into_response()
}

// ── Team management ───────────────────────────────────────────────────────────

/// `GET /api/v1/teams` — list all teams.
async fn list_teams_handler(_state: State<Arc<GatewayState>>) -> impl IntoResponse {
    match crate::team::Team::list_all().await {
        Ok(names) => {
            Json(serde_json::json!({ "teams": names, "count": names.len() })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct CreateTeamRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

/// `POST /api/v1/teams` — create a new team.
async fn create_team_handler(
    _state: State<Arc<GatewayState>>,
    Json(req): Json<CreateTeamRequest>,
) -> impl IntoResponse {
    let mut team = crate::team::Team::new(req.name.clone());
    team.description = req.description;
    team.active = true;

    match team.save().await {
        Ok(()) => (StatusCode::CREATED, Json(team)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{}", e) })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/teams/:id` — get team details.
async fn get_team_handler(
    Path(id): Path<String>,
    _state: State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(team) => Json(team).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

/// `DELETE /api/v1/teams/:id` — delete a team.
async fn delete_team_handler(
    Path(id): Path<String>,
    _state: State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(team) => match team.delete().await {
            Ok(()) => Json(serde_json::json!({ "success": true, "name": id })).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{}", e) })),
            )
                .into_response(),
        },
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

/// `GET /api/v1/teams/:id/members` — list team members.
async fn list_team_members_handler(
    Path(id): Path<String>,
    _state: State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(team) => {
            let members: Vec<_> = team.members.values().collect();
            Json(serde_json::json!({ "members": members, "count": members.len() })).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct AddTeamMemberRequest {
    agent: String,
    #[serde(default = "default_member_role")]
    role: String,
}

fn default_member_role() -> String {
    "member".to_string()
}

/// `POST /api/v1/teams/:id/members` — add a member to the team.
async fn add_team_member_handler(
    Path(id): Path<String>,
    _state: State<Arc<GatewayState>>,
    Json(req): Json<AddTeamMemberRequest>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(mut team) => {
            team.add_member(req.agent.clone(), req.role);
            match team.save().await {
                Ok(()) => Json(serde_json::json!({
                    "success": true,
                    "team": id,
                    "agent": req.agent,
                }))
                .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                )
                    .into_response(),
            }
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

/// `DELETE /api/v1/teams/:id/members/:agent` — remove a member from the team.
async fn remove_team_member_handler(
    Path((id, agent)): Path<(String, String)>,
    _state: State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match crate::team::Team::load(&id).await {
        Ok(mut team) => {
            team.remove_member(&agent);
            match team.save().await {
                Ok(()) => Json(serde_json::json!({
                    "success": true,
                    "team": id,
                    "agent": agent,
                }))
                .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                )
                    .into_response(),
            }
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct AssignTeamTaskRequest {
    task: String,
    #[serde(default = "default_task_priority")]
    priority: String,
}

fn default_task_priority() -> String {
    "normal".to_string()
}

/// `POST /api/v1/teams/:id/tasks` — assign a task to the team via the mesh.
async fn assign_team_task_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AssignTeamTaskRequest>,
) -> impl IntoResponse {
    // Verify team exists
    let team = match crate::team::Team::load(&id).await {
        Ok(t) => t,
        Err(e) => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                .into_response();
        }
    };

    // Route the task through the inbound pipeline using the team as a session
    let session_id = format!("team:{}", id);
    let incoming = crate::channels::IncomingMessage::new(
        format!("team:{}", team.name),
        session_id,
        format!("[priority:{}] {}", req.priority, req.task),
    )
    .with_provenance(crate::channels::InputProvenance::InternalSystem {
        source: "team".to_string(),
    });
    let _ = state.inbound_pipeline.process(incoming).await;

    Json(serde_json::json!({
        "success": true,
        "team": id,
        "task": req.task,
        "priority": req.priority,
        "queued": true,
    }))
    .into_response()
}

// ── Session / Thread / Turn API ───────────────────────────────────────────────

/// `GET /api/sessions` — list all active sessions and their routing info.
async fn list_sessions_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let bindings = state.agent_router.list_bindings().await;
    let sessions: Vec<_> = bindings
        .iter()
        .map(|(session_id, (agent_id, workspace_id))| {
            serde_json::json!({
                "session_id": session_id,
                "agent_id": agent_id,
                "workspace_id": workspace_id,
            })
        })
        .collect();
    let count = sessions.len();
    Json(serde_json::json!({
        "sessions": sessions,
        "count": count,
    }))
}

/// Resolve session_id → query sender, returning a 404 response on failure.
///
/// The caller must NOT hold any lock when invoking this helper.
async fn resolve_session_query_tx(
    state: &Arc<GatewayState>,
    session_id: &str,
) -> Result<mpsc::Sender<AgentQuery>, axum::response::Response> {
    let agent_id = {
        let route = state.agent_router.resolve_by_session(session_id).await;
        if route.agent_id == "default" && route.created_binding {
            // No existing binding and fell back to default — treat as not found
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Session '{}' not found", session_id)
                })),
            )
                .into_response());
        }
        route.agent_id
    };

    let agents = state.agents.read().await;
    match agents.get(&agent_id) {
        Some(handle) => Ok(handle.query_tx.clone()),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("Agent '{}' for session '{}' not found", agent_id, session_id)
            })),
        )
            .into_response()),
    }
}

/// `GET /api/sessions/:id/threads` — list threads for a session's agent.
async fn list_threads_handler(
    Path(session_id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let qtx = match resolve_session_query_tx(&state, &session_id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if qtx
        .send(AgentQuery::GetThreadSummaries { response_tx: resp_tx })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "agent unavailable"})),
        )
            .into_response();
    }
    let summaries = match resp_rx.await {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "agent response channel closed"})),
            )
                .into_response()
        }
    };

    let threads: Vec<_> = summaries
        .into_iter()
        .map(|(thread_id, label, turn_count, conv_id)| {
            serde_json::json!({
                "thread_id": thread_id,
                "label": label,
                "turn_count": turn_count,
                "conversation_id": conv_id,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "session_id": session_id,
            "threads": threads,
        })),
    )
        .into_response()
}

/// `GET /api/sessions/:id/threads/:thread_id/turns` — list turns for a thread.
async fn list_turns_handler(
    Path((session_id, thread_id)): Path<(String, String)>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let qtx = match resolve_session_query_tx(&state, &session_id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };

    // Thread map key is `conversation_id`; the CLI passes `thread_id` with a
    // "thread-" prefix. Strip it to get the correct map key.
    let conv_id = thread_id
        .strip_prefix("thread-")
        .unwrap_or(&thread_id)
        .to_string();

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if qtx
        .send(AgentQuery::GetThreadTurns { conv_id, response_tx: resp_tx })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "agent unavailable"})),
        )
            .into_response();
    }
    match resp_rx.await {
        Ok(Some(turns)) => {
            let turns_json: Vec<_> = turns
                .into_iter()
                .map(|(index, turn_state, user_preview, asst_preview)| {
                    serde_json::json!({
                        "index": index,
                        "state": turn_state,
                        "user_preview": user_preview,
                        "assistant_preview": asst_preview,
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "session_id": session_id,
                    "thread_id": thread_id,
                    "turns": turns_json,
                })),
            )
                .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("Thread '{}' not found", thread_id),
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "agent response channel closed"})),
        )
            .into_response(),
    }
}

/// `POST /api/sessions/:id/threads/:thread_id/undo` — undo the last turn of a thread.
async fn undo_turn_handler(
    Path((session_id, thread_id)): Path<(String, String)>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let qtx = match resolve_session_query_tx(&state, &session_id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };

    let conv_id = thread_id
        .strip_prefix("thread-")
        .unwrap_or(&thread_id)
        .to_string();

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if qtx
        .send(AgentQuery::UndoLastTurn { conv_id, response_tx: resp_tx })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "agent unavailable"})),
        )
            .into_response();
    }
    match resp_rx.await {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "session_id": session_id,
                "thread_id": thread_id,
                "message": "Last turn undone successfully",
            })),
        )
            .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "Thread '{}' not found or has no turns to undo",
                    thread_id
                ),
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "agent response channel closed"})),
        )
            .into_response(),
    }
}

/// `POST /api/sessions/:id/threads/:thread_id/redo` — redo the most recently undone turn.
async fn redo_turn_handler(
    Path((session_id, thread_id)): Path<(String, String)>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let qtx = match resolve_session_query_tx(&state, &session_id).await {
        Ok(tx) => tx,
        Err(resp) => return resp,
    };

    let conv_id = thread_id
        .strip_prefix("thread-")
        .unwrap_or(&thread_id)
        .to_string();

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if qtx
        .send(AgentQuery::RedoLastTurn { conv_id, response_tx: resp_tx })
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "agent unavailable"})),
        )
            .into_response();
    }
    match resp_rx.await {
        Ok(true) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "session_id": session_id,
                "thread_id": thread_id,
                "message": "Turn redone successfully",
            })),
        )
            .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "Thread '{}' not found or has no turns to redo",
                    thread_id
                ),
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "agent response channel closed"})),
        )
            .into_response(),
    }
}

/// SSE events handler for web terminal
/// Streams gateway events to the browser in the format expected by the web UI
#[allow(dead_code)]
async fn web_terminal_events_handler(
    State(state): State<Arc<GatewayState>>,
) -> axum::response::sse::Sse<
    impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures_util::StreamExt;
    use tokio_stream::wrappers::BroadcastStream;

    // Subscribe to gateway events
    let rx = state.event_tx.subscribe();

    let stream = BroadcastStream::new(rx).filter_map(|result| async move {
        match result {
            Ok(evt) => {
                // Serialize GatewayEvent directly - let terminals handle display logic
                // Add event_type field to help terminals identify event type
                let mut json_value = serde_json::to_value(&evt).unwrap_or_default();
                if let serde_json::Value::Object(ref mut map) = json_value {
                    // Add event_type field based on the variant
                    let event_type = match &evt {
                        GatewayEvent::AgentResponse { .. } => "agent_response",
                        GatewayEvent::Thinking { .. } => "thinking",
                        GatewayEvent::ContentDelta { .. } => "content_delta",
                        GatewayEvent::ToolCalling { .. } => "tool_calling",
                        GatewayEvent::ToolResult { .. } => "tool_result",
                        GatewayEvent::AgentStatus { .. } => "agent_status",
                        GatewayEvent::ProcessingError { .. } => "processing_error",
                        GatewayEvent::Completed { .. } => "completed",
                        GatewayEvent::MessageReceived { .. } => "message_received",
                        GatewayEvent::ChannelStatus { .. } => "channel_status",
                        GatewayEvent::ApprovalRequired { .. } => "approval_required",
                        GatewayEvent::RepairAction { .. } => "repair_action",
                        GatewayEvent::DevicePairRequested { .. } => "device_pair_requested",
                        GatewayEvent::SessionCreated { .. } => "session_created",
                        GatewayEvent::SessionRenamed { .. } => "session_renamed",
                        GatewayEvent::CronAnnounce { .. } => "cron_announce",
                    };
                    map.insert("event_type".to_string(), serde_json::json!(event_type));
                }
                let data = json_value.to_string();
                Some(Ok(Event::default().data(data)))
            }
            Err(_) => None,
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// List all registered event hooks
async fn list_hooks_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let hooks = state.hook_registry.list_hooks().await;
    Json(hooks)
}

/// Unregister a hook by name
async fn unregister_hook_handler(
    State(state): State<Arc<GatewayState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let removed = state.hook_registry.unregister(&name).await;
    if removed {
        (StatusCode::OK, Json(serde_json::json!({"status": "removed", "name": name})))
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Hook not found", "name": name})),
        )
            .into_response()
    }
}

/// Get current gateway configuration
async fn get_config_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let config = state.config.read().await;
    match serde_json::to_value(&*config) {
        Ok(json) => (StatusCode::OK, Json(json)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Serialization failed: {}", e)})),
        )
            .into_response(),
    }
}

/// Update gateway configuration and persist to disk
async fn put_config_handler(
    State(state): State<Arc<GatewayState>>,
    Json(new_config): Json<GatewayConfig>,
) -> impl IntoResponse {
    let config_path = match state.config_path.clone() {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(
                    serde_json::json!({"error": "No config file path configured — cannot persist changes"}),
                ),
            )
                .into_response();
        }
    };

    // Serialize to TOML
    let toml_str = match toml::to_string_pretty(&new_config) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": format!("TOML serialization failed: {}", e)})),
            )
                .into_response();
        }
    };

    // Write to disk
    if let Err(e) = tokio::fs::write(&config_path, toml_str).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to write config file: {}", e)})),
        )
            .into_response();
    }

    // Update in-memory config
    {
        let mut config = state.config.write().await;
        *config = new_config;
    }

    info!("Config updated and persisted to {:?}", config_path);

    state
        .audit_log
        .log(
            crate::security::runtime_audit::AuditEventType::ConfigChange,
            "admin",
            "gateway",
            true,
            format!("Config updated and persisted to {}", config_path.display()),
            Some(serde_json::json!({"path": config_path.to_string_lossy()})),
        )
        .await;

    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "updated", "path": config_path.to_string_lossy()})),
    )
        .into_response()
}

/// Validate a configuration without persisting it
async fn validate_config_handler(Json(config): Json<GatewayConfig>) -> impl IntoResponse {
    // Basic validation: try to serialize and deserialize as TOML
    match toml::to_string(&config) {
        Ok(toml_str) => {
            match toml::from_str::<GatewayConfig>(&toml_str) {
                Ok(_) => (
                    StatusCode::OK,
                    Json(serde_json::json!({"valid": true, "message": "Configuration is valid"})),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"valid": false, "error": format!("TOML deserialization failed: {}", e)})),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"valid": false, "error": format!("TOML serialization failed: {}", e)})),
        )
            .into_response(),
    }
}

// ── Pairing / DM Access Control Handlers ───────────────────────────────────

#[derive(Debug, Deserialize)]
struct PairingChannelQuery {
    channel: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApprovePairingRequest {
    channel: String,
    code: String,
}

#[derive(Debug, Deserialize)]
struct RejectPairingRequest {
    channel: String,
    code: String,
}

#[derive(Debug, Deserialize)]
struct RevokePairingRequest {
    channel: String,
    user_id: String,
}

#[derive(Debug, Deserialize)]
struct AddAllowlistRequest {
    channel: String,
    user_id: String,
    username: Option<String>,
}

/// `GET /api/v1/pairing/pending` — list pending pairing requests.
async fn list_pairing_pending_handler(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<PairingChannelQuery>,
) -> impl IntoResponse {
    let pending = if let Some(channel) = query.channel {
        state.pairing_store.list_pending(&channel).await
    } else {
        // List all pending across all channels
        let mut all = Vec::new();
        let channels = {
            let cfg = state.config.read().await;
            cfg.channels.keys().cloned().collect::<Vec<_>>()
        };
        for channel in channels {
            let mut channel_pending = state.pairing_store.list_pending(&channel).await;
            all.append(&mut channel_pending);
        }
        all
    };
    Json(pending)
}

/// `GET /api/v1/pairing/authorized` — list authorized users.
async fn list_pairing_authorized_handler(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<PairingChannelQuery>,
) -> impl IntoResponse {
    let authorized = if let Some(channel) = query.channel {
        state
            .pairing_store
            .list_authorized_for_channel(&channel)
            .await
    } else {
        state.pairing_store.list_authorized().await
    };
    Json(authorized)
}

/// `POST /api/v1/pairing/approve` — approve a pending request by code.
async fn approve_pairing_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<ApprovePairingRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    match state
        .pairing_store
        .approve(&req.channel, &req.code, Some("admin"))
        .await
    {
        Some(user) => {
            state
                .audit_log
                .log(
                    AuditEventType::PairingApprove,
                    "admin",
                    &req.channel,
                    true,
                    format!("Approved user {} on channel {}", user.user_id, user.channel),
                    Some(serde_json::json!({"user_id": user.user_id, "code": req.code})),
                )
                .await;
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "approved",
                    "user_id": user.user_id,
                    "channel": user.channel,
                })),
            )
                .into_response()
        }
        None => {
            state
                .audit_log
                .log(
                    AuditEventType::PairingApprove,
                    "admin",
                    &req.channel,
                    false,
                    format!("Approve failed: code {} not found or expired", req.code),
                    None,
                )
                .await;
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Pairing request not found or expired",
                    "code": req.code,
                    "channel": req.channel,
                })),
            )
                .into_response()
        }
    }
}

/// `POST /api/v1/pairing/reject` — reject a pending request by code.
async fn reject_pairing_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<RejectPairingRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    match state.pairing_store.reject(&req.channel, &req.code).await {
        Some(r) => {
            state
                .audit_log
                .log(
                    AuditEventType::PairingReject,
                    "admin",
                    &req.channel,
                    true,
                    format!("Rejected user {} on channel {}", r.user_id, r.channel),
                    Some(serde_json::json!({"user_id": r.user_id, "code": req.code})),
                )
                .await;
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "rejected",
                    "user_id": r.user_id,
                    "channel": r.channel,
                })),
            )
                .into_response()
        }
        None => {
            state
                .audit_log
                .log(
                    AuditEventType::PairingReject,
                    "admin",
                    &req.channel,
                    false,
                    format!("Reject failed: code {} not found", req.code),
                    None,
                )
                .await;
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Pairing request not found",
                    "code": req.code,
                    "channel": req.channel,
                })),
            )
                .into_response()
        }
    }
}

/// `POST /api/v1/pairing/revoke` — revoke an authorized user.
async fn revoke_pairing_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<RevokePairingRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    let removed = state.pairing_store.revoke(&req.channel, &req.user_id).await;
    if removed {
        state
            .audit_log
            .log(
                AuditEventType::PairingRevoke,
                "admin",
                &req.channel,
                true,
                format!("Revoked user {} on channel {}", req.user_id, req.channel),
                Some(serde_json::json!({"user_id": req.user_id})),
            )
            .await;
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "revoked",
                "user_id": req.user_id,
                "channel": req.channel,
            })),
        )
            .into_response()
    } else {
        state
            .audit_log
            .log(
                AuditEventType::PairingRevoke,
                "admin",
                &req.channel,
                false,
                format!("Revoke failed: user {} not found in authorized list", req.user_id),
                None,
            )
            .await;
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "User not found in authorized list",
                "user_id": req.user_id,
                "channel": req.channel,
            })),
        )
            .into_response()
    }
}

/// `POST /api/v1/pairing/allowlist` — add a user directly to the allowlist.
async fn add_allowlist_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AddAllowlistRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    let user = state
        .pairing_store
        .add_to_allowlist(&req.channel, &req.user_id, req.username.as_deref(), Some("admin"))
        .await;
    state
        .audit_log
        .log(
            AuditEventType::PairingApprove,
            "admin",
            &req.channel,
            true,
            format!("Added user {} to allowlist on channel {}", req.user_id, req.channel),
            Some(serde_json::json!({"user_id": req.user_id, "username": req.username})),
        )
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "added",
            "user_id": user.user_id,
            "channel": user.channel,
        })),
    )
        .into_response()
}

// ── Command Gate Handlers ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SetGateLevelRequest {
    user_id: String,
    level: String,
}

/// `GET /api/v1/gate/levels` — list all configured user levels.
async fn list_gate_levels_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let levels = state.command_gate.user_levels();
    let json_levels: std::collections::HashMap<String, String> = levels
        .into_iter()
        .map(|(k, v)| (k, v.to_string()))
        .collect();
    Json(serde_json::json!({
        "levels": json_levels,
        "default": "chat",
    }))
}

/// `POST /api/v1/gate/levels` — set a user's permission level.
async fn set_gate_level_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SetGateLevelRequest>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    let level = match req.level.as_str() {
        "chat" => crate::tools::command_gate::UserLevel::Chat,
        "user" => crate::tools::command_gate::UserLevel::User,
        "admin" => crate::tools::command_gate::UserLevel::Admin,
        _ => {
            state
                .audit_log
                .log(
                    AuditEventType::CommandGate,
                    "admin",
                    "gateway",
                    false,
                    format!("Invalid level '{}' for user {}", req.level, req.user_id),
                    None,
                )
                .await;
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("Invalid level '{}'. Expected: chat, user, admin", req.level)
                })),
            )
                .into_response();
        }
    };

    state.command_gate.set_user_level(&req.user_id, level);
    state
        .audit_log
        .log(
            AuditEventType::CommandGate,
            "admin",
            "gateway",
            true,
            format!("Set user {} level to {}", req.user_id, req.level),
            Some(serde_json::json!({"user_id": req.user_id, "level": req.level})),
        )
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "updated",
            "user_id": req.user_id,
            "level": req.level,
        })),
    )
        .into_response()
}

/// `DELETE /api/v1/gate/levels/:user_id` — clear a user's custom level.
async fn clear_gate_level_handler(
    State(state): State<Arc<GatewayState>>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;
    state.command_gate.clear_user_level(&user_id);
    state
        .audit_log
        .log(
            AuditEventType::CommandGate,
            "admin",
            "gateway",
            true,
            format!("Cleared custom level for user {}", user_id),
            Some(serde_json::json!({"user_id": user_id})),
        )
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "cleared",
            "user_id": user_id,
        })),
    )
        .into_response()
}

// ── Mention Gate Handlers ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SetMentionPolicyRequest {
    policy: crate::security::mention_gate::MentionPolicy,
}

#[derive(Debug, Deserialize)]
struct AddMentionPatternRequest {
    channel: String,
    pattern: String,
}

/// `GET /api/v1/mentions/policy` — get current mention gate policy.
async fn get_mention_policy_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let policy = state.mention_gate.policy().await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "policy": policy.to_string(),
        })),
    )
        .into_response()
}

/// `POST /api/v1/mentions/policy` — set mention gate policy.
async fn set_mention_policy_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SetMentionPolicyRequest>,
) -> impl IntoResponse {
    state.mention_gate.set_policy(req.policy).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "ok",
            "policy": req.policy.to_string(),
        })),
    )
        .into_response()
}

/// `GET /api/v1/mentions/allowlist` — list allowlist entries for a channel.
async fn list_mention_allowlist_handler(
    State(state): State<Arc<GatewayState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let channel = params
        .get("channel")
        .cloned()
        .unwrap_or_else(|| "*".to_string());
    let entries = state.mention_gate.list_allowlist(&channel).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "channel": channel,
            "allowlist": entries,
        })),
    )
        .into_response()
}

/// `POST /api/v1/mentions/allowlist` — add a pattern to the allowlist.
async fn add_mention_allowlist_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AddMentionPatternRequest>,
) -> impl IntoResponse {
    state
        .mention_gate
        .add_allowlist(&req.channel, &req.pattern)
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "added",
            "channel": req.channel,
            "pattern": req.pattern,
        })),
    )
        .into_response()
}

/// `DELETE /api/v1/mentions/allowlist/:channel/:pattern` — remove from allowlist.
async fn remove_mention_allowlist_handler(
    State(state): State<Arc<GatewayState>>,
    Path((channel, pattern)): Path<(String, String)>,
) -> impl IntoResponse {
    let removed = state
        .mention_gate
        .remove_allowlist(&channel, &pattern)
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": if removed { "removed" } else { "not_found" },
            "channel": channel,
            "pattern": pattern,
        })),
    )
        .into_response()
}

/// `GET /api/v1/mentions/blocklist` — list blocklist entries for a channel.
async fn list_mention_blocklist_handler(
    State(state): State<Arc<GatewayState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let channel = params
        .get("channel")
        .cloned()
        .unwrap_or_else(|| "*".to_string());
    let entries = state.mention_gate.list_blocklist(&channel).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "channel": channel,
            "blocklist": entries,
        })),
    )
        .into_response()
}

/// `POST /api/v1/mentions/blocklist` — add a pattern to the blocklist.
async fn add_mention_blocklist_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AddMentionPatternRequest>,
) -> impl IntoResponse {
    state
        .mention_gate
        .add_blocklist(&req.channel, &req.pattern)
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "added",
            "channel": req.channel,
            "pattern": req.pattern,
        })),
    )
        .into_response()
}

/// `DELETE /api/v1/mentions/blocklist/:channel/:pattern` — remove from blocklist.
async fn remove_mention_blocklist_handler(
    State(state): State<Arc<GatewayState>>,
    Path((channel, pattern)): Path<(String, String)>,
) -> impl IntoResponse {
    let removed = state
        .mention_gate
        .remove_blocklist(&channel, &pattern)
        .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": if removed { "removed" } else { "not_found" },
            "channel": channel,
            "pattern": pattern,
        })),
    )
        .into_response()
}

// ── Audit Log Handler ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AuditLogQuery {
    limit: Option<usize>,
    event_type: Option<String>,
}

/// `GET /api/v1/audit/log` — retrieve recent audit log entries.
async fn list_audit_log_handler(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<AuditLogQuery>,
) -> impl IntoResponse {
    use crate::security::runtime_audit::AuditEventType;

    let entries = if let Some(ref etype) = query.event_type {
        let event_type = match etype.as_str() {
            "access_check" => AuditEventType::AccessCheck,
            "pairing_request" => AuditEventType::PairingRequest,
            "pairing_approve" => AuditEventType::PairingApprove,
            "pairing_reject" => AuditEventType::PairingReject,
            "pairing_revoke" => AuditEventType::PairingRevoke,
            "command_gate" => AuditEventType::CommandGate,
            "config_change" => AuditEventType::ConfigChange,
            "tool_invocation" => AuditEventType::ToolInvocation,
            "tool_deny" => AuditEventType::ToolDeny,
            "security" => AuditEventType::Security,
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": format!("Unknown event_type: {}", etype)
                    })),
                )
                    .into_response();
            }
        };
        state.audit_log.filter(event_type).await
    } else {
        state.audit_log.recent(query.limit.unwrap_or(100)).await
    };

    Json(serde_json::json!({
        "entries": entries,
        "count": entries.len(),
    }))
    .into_response()
}

#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod state_tests;
