//! Gateway Control Plane
//!
//! The Gateway is the control plane for Manta, managing:
//! - Multi-channel message routing (WhatsApp, Telegram, Feishu, etc.)
//! - Session management and routing to agents
//! - Agent spawning and lifecycle management
//! - WebSocket/HTTP API for channel adapters
//! - Authentication and security policies

use axum::{
    extract::{ConnectInfo, Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    middleware::{from_fn, from_fn_with_state},
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, error, info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::{Agent, AgentConfig};
use crate::canvas::{CanvasEvent, CanvasManager};
use crate::channels::{Channel, ChannelType};
use crate::config::hot_reload::{ConfigFileType, HotReloadManager};
use crate::memory::vector::{
    ApiEmbeddingProvider, EmbeddingConfig, LocalGgufEmbeddingProvider, MemoryVectorStore,
    VectorMemoryService,
};
use crate::model_router::ModelRouter;
use crate::plugins::PluginManager;
use crate::tools::ToolRegistry;

pub mod middleware;
pub mod webhooks;

/// Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Host to bind to
    pub host: String,
    /// Port for gateway control plane
    pub port: u16,
    /// Port for web terminal
    pub web_port: u16,
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
}

fn default_model() -> String {
    "claude-3-sonnet-20240229".to_string()
}

fn default_model_provider() -> String {
    "anthropic".to_string()
}

/// Embedding provider type
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_subagents: 10,
            default_timeout_seconds: 300,
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

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Enable security features (auth, rate limiting, security headers)
    pub enabled: bool,
    /// Require authentication for API access
    pub auth_required: bool,
    /// Require pairing for new users
    pub pairing_required: bool,
    /// Rate limiting configuration
    pub rate_limit: RateLimitConfig,
    /// Enable security headers
    pub security_headers: bool,
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting
    pub enabled: bool,
    /// Maximum requests per window
    pub capacity: u32,
    /// Refill rate (tokens per second)
    pub refill_rate: f64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auth_required: false,
            pairing_required: false,
            rate_limit: RateLimitConfig::default(),
            security_headers: true,
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            capacity: 100,
            refill_rate: 10.0,
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
            web_port: 18081,
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
        }
    }
}

/// Channel-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Channel type
    pub channel_type: ChannelType,
    /// Whether channel is enabled
    pub enabled: bool,
    /// Channel-specific credentials/tokens
    pub credentials: HashMap<String, String>,
    /// DM policy: "open" | "pairing" | "blocked"
    pub dm_policy: String,
    /// Allowlist of users/numbers
    pub allow_from: Vec<String>,
    /// Blocklist of users/numbers
    pub block_from: Vec<String>,
    /// Agent ID to route to (None = default)
    pub agent_id: Option<String>,
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
    pub session_routing: Arc<RwLock<HashMap<String, String>>>,
    /// Model router for multi-provider support
    pub model_router: Arc<ModelRouter>,
    /// Tool registry for all agents
    pub tool_registry: Arc<ToolRegistry>,
    /// Event broadcast channel
    pub event_tx: broadcast::Sender<GatewayEvent>,
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
    /// Hot reload manager for config changes (RwLock for late initialization)
    pub hot_reload: RwLock<Option<Arc<HotReloadManager>>>,
    /// Cron scheduler for scheduled jobs (RwLock for late initialization)
    pub cron_scheduler: RwLock<Option<Arc<tokio::sync::Mutex<crate::cron::CronScheduler>>>>,
    /// Auth manager for authentication
    pub auth_manager: Arc<crate::security::AuthManager>,
    /// Rate limiter for API protection
    pub rate_limiter: Arc<crate::security::RateLimiter>,
    /// Storage adapter for persistence
    pub storage: Arc<RwLock<dyn crate::adapters::Storage>>,
    /// Skills manager for hot-reloadable skills
    pub skills_manager: Arc<RwLock<crate::skills::SkillManager>>,
}

/// Handle to a running agent
#[derive(Clone)]
pub struct AgentHandle {
    /// Agent ID
    pub id: String,
    /// Agent configuration
    pub config: AgentConfig,
    /// Communication channel
    pub tx: mpsc::Sender<AgentCommand>,
    /// Whether agent is currently processing
    pub busy: bool,
    /// The actual agent instance
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
    },
    /// Cancel current operation
    Cancel,
    /// Update configuration
    UpdateConfig(AgentConfig),
    /// Shutdown agent
    Shutdown,
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
}

/// Agent status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentStatus {
    Idle,
    Processing { session_id: String },
    Error(String),
    Shutdown,
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
    pub async fn new(config: GatewayConfig) -> crate::Result<Self> {
        let (event_tx, _) = broadcast::channel(1000);
        let (message_queue_tx, message_queue_rx) = mpsc::channel(1000);

        // Create ACP control plane first (needed for tool registration)
        let acp = Arc::new(AcpControlPlane::new());

        // Create tool registry with built-in tools (including ACP tools if enabled)
        let tool_registry = Arc::new(create_default_tool_registry(acp.clone()).await?);

        // Initialize plugin manager
        let plugins_dir = crate::dirs::config_dir().join("plugins");
        let plugin_manager = Arc::new(PluginManager::new(plugins_dir).await?);

        // Create model router config with custom model settings
        let mut model_router_config = crate::model_router::ModelRouterConfig::default();
        model_router_config.default_model = "default".to_string();
        // Update the default alias to use the configured model and provider
        if let Some(default_alias) = model_router_config.aliases.get_mut("default") {
            default_alias.provider = config.model_provider.clone();
            default_alias.model = config.model.clone();
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

        // Initialize storage adapter
        // For unified storage (sqlite), we keep a separate Arc to use for VectorStore/MemoryStore
        let (storage, unified_vector_store): (
            Arc<RwLock<dyn crate::adapters::Storage>>,
            Option<Arc<dyn crate::memory::VectorStore>>,
        ) = match config.storage.storage_type.as_str() {
            "sqlite" => {
                // Use absolute path for database to avoid working directory issues
                let db_path = config
                    .storage
                    .database_url
                    .as_ref()
                    .map(|s| std::path::PathBuf::from(s.strip_prefix("sqlite:").unwrap_or(s)))
                    .unwrap_or_else(|| crate::dirs::manta_dir().join("data").join("manta.db"));

                // Ensure parent directory exists
                if let Some(parent) = db_path.parent() {
                    tokio::fs::create_dir_all(parent).await.ok();
                }

                // SQLite URL format: sqlite:///absolute/path/to/db for absolute paths
                let db_url = format!("sqlite:///{}", db_path.display());
                info!("Connecting to SQLite storage at: {}", db_url);

                let sqlite_storage =
                    Arc::new(crate::adapters::SqliteStorage::connect(&db_url).await?);
                // Clone the Arc for use as VectorStore trait object
                let vector_store: Arc<dyn crate::memory::VectorStore> = sqlite_storage.clone();
                // Wrap in RwLock for the generic storage interface
                let storage: Arc<RwLock<dyn crate::adapters::Storage>> =
                    Arc::new(RwLock::new(crate::adapters::SqliteStorage::connect(&db_url).await?));
                (storage, Some(vector_store))
            }
            "file" => {
                let base_path = config.storage.base_path.as_deref().unwrap_or("./data");
                let storage = Arc::new(RwLock::new(crate::adapters::FileStorage::new(base_path)?));
                (storage, None)
            }
            _ => {
                // Default to memory storage
                let storage = Arc::new(RwLock::new(crate::adapters::InMemoryStorage::new()));
                (storage, None)
            }
        };

        // Create state with placeholder values for vector_memory and hot_reload
        // We'll fill them in after state creation to allow callbacks to reference state
        let state = Arc::new(GatewayState {
            config: Arc::new(RwLock::new(config.clone())),
            channels: Arc::new(RwLock::new(HashMap::new())),
            agents: Arc::new(RwLock::new(HashMap::new())),
            session_routing: Arc::new(RwLock::new(HashMap::new())),
            model_router: Arc::new(ModelRouter::new(model_router_config)),
            tool_registry,
            event_tx,
            message_queue: message_queue_tx,
            canvas_manager: Arc::new(CanvasManager::new()),
            plugin_manager,
            acp,
            vector_memory: RwLock::new(None),
            hot_reload: RwLock::new(None),
            cron_scheduler: RwLock::new(None),
            auth_manager,
            rate_limiter,
            storage,
            skills_manager: Arc::new(RwLock::new(crate::skills::SkillManager::new().await?)),
        });

        // Configure providers from config
        for (name, provider_config) in &config.providers {
            info!("Configuring provider: {}", name);
            if let Err(e) = state
                .model_router
                .add_provider(name, provider_config.clone())
                .await
            {
                warn!("Failed to add provider '{}': {}", name, e);
            }
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

                let service = Arc::new(VectorMemoryService::new(
                    embedding_provider,
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
            info!("Initializing cron scheduler...");
            use crate::cron::CronScheduler;
            let (cron_scheduler, command_rx) = CronScheduler::new();
            let cron_scheduler = Arc::new(tokio::sync::Mutex::new(cron_scheduler));
            // Start the scheduler in a background task
            let cron_scheduler_clone = Arc::clone(&cron_scheduler);
            tokio::spawn(async move {
                let mut scheduler = cron_scheduler_clone.lock().await;
                if let Err(e) = scheduler.start(command_rx).await {
                    warn!("Cron scheduler failed: {}", e);
                }
            });
            *state.cron_scheduler.write().await = Some(cron_scheduler);
            info!("✅ Cron scheduler initialized");
        } else {
            info!("Cron scheduler disabled");
        }

        // Start message processing worker
        tokio::spawn(Self::process_message_queue(state.clone(), message_queue_rx));

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

        // Initialize configured channels
        self.init_channels().await?;

        // Build HTTP router
        let app = self.build_router();

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

        // Start web terminal server
        tokio::spawn(Self::start_web_terminal(self.config.web_port, self.state.clone()));

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
    fn build_router(&self) -> Router {
        let state = self.state.clone();

        // Public tier: Webhooks (no authentication, signature verification per-channel)
        let public_router = webhooks::create_webhook_router(state.clone());

        // Admin tier: Protected APIs (localhost/Tailscale only)
        let admin_router = Router::new()
            // Health check (public)
            .route("/health", get(health_handler))
            // Simple chat endpoint (backwards compatibility with DaemonClient)
            .route("/chat", post(chat_handler))
            // WebSocket endpoints (localhost/Tailscale only)
            .route("/ws", get(ws_handler))
            .route("/ws/canvas/:id", get(canvas_ws_handler))
            // Agent management
            .route("/api/v1/agents", get(list_agents_handler).post(create_agent_handler))
            .route(
                "/api/v1/agents/:id",
                get(get_agent_handler).delete(delete_agent_handler),
            )
            // Channel management
            .route("/api/v1/channels", get(list_channels_handler))
            // Session messaging with provider override
            .route("/api/v1/sessions/:id/messages", post(send_message_handler))
            // Conversation history
            .route("/api/v1/conversations/:id/messages", get(get_conversation_history_handler))
            .route("/api/v1/conversations/last", get(get_last_conversation_handler))
            // Status
            .route("/api/v1/status", get(status_handler))
            // Canvas/A2UI
            .route("/api/v1/canvas", post(create_canvas_handler))
            .route("/api/v1/canvas/:id", get(get_canvas_handler).delete(delete_canvas_handler))
            // Provider management (runtime switching)
            .route("/api/v1/providers", get(list_providers_handler))
            .route("/api/v1/providers/switch", post(switch_model_handler))
            .route("/api/v1/providers/:id/health", get(get_provider_health_handler))
            .route("/api/v1/providers/:id/enable", post(enable_provider_handler))
            .route("/api/v1/providers/:id/disable", post(disable_provider_handler))
            .route("/api/v1/providers/:id/check", post(check_provider_handler))
            .route("/api/v1/providers/fallback/:alias", get(get_fallback_chain_handler).post(set_fallback_chain_handler))
            // Model aliases
            .route("/api/v1/models", get(list_models_handler))
            .route("/api/v1/models/default", get(get_default_model_handler))
            // Vector Memory API
            .route("/api/v1/memory/search", post(memory_search_handler))
            .route("/api/v1/memory/add", post(memory_add_handler))
            .route("/api/v1/memory/collections", get(list_memory_collections_handler))
            // Plugin management API
            .route("/api/v1/plugins", get(list_plugins_handler))
            .route("/api/v1/plugins/:id/enable", post(enable_plugin_handler))
            .route("/api/v1/plugins/:id/disable", post(disable_plugin_handler))
            .route("/api/v1/plugins/:id/unload", delete(unload_plugin_handler))
            // Skills API
            .route("/api/v1/skills", get(list_skills_handler))
            .route("/api/v1/skills/:id", get(get_skill_handler))
            .route("/api/v1/skills/:id/enable", post(enable_skill_handler))
            .route("/api/v1/skills/:id/disable", post(disable_skill_handler))
            .route("/api/v1/skills/:id/run", post(run_skill_handler))
            // ACP (Agent Control Plane) API
            .route("/api/v1/acp/sessions", get(list_acp_sessions_handler))
            .route("/api/v1/acp/sessions", post(spawn_subagent_handler))
            .route("/api/v1/acp/sessions/:id", delete(terminate_acp_session_handler))
            .route("/api/v1/acp/sessions/:id/message", post(acp_session_message_handler))
            // Apply security middleware (order matters - applied in reverse)
            .layer(from_fn_with_state(state.clone(), middleware::rate_limit_middleware))
            .layer(from_fn_with_state(state.clone(), middleware::auth_middleware))
            .layer(from_fn(middleware::tailscale_only_middleware))
            .layer(from_fn(middleware::security_headers_middleware))
            .with_state(state.clone());

        // Merge public and admin routers
        public_router.merge(admin_router)
    }

    /// Spawn a new agent
    async fn spawn_agent(&self, id: String, config: AgentConfig) -> crate::Result<()> {
        info!("Spawning agent: {}", id);

        let (tx, mut rx) = mpsc::channel(100);

        // Create provider from model router
        let provider: Arc<dyn crate::providers::Provider> =
            self.state.model_router.create_default_provider().await?;

        // Get tool registry from state
        let tools = self.state.tool_registry.clone();

        // Get the model from config for this agent
        let model = self.state.config.read().await.model.clone();

        // Create the actual Agent instance with model
        let agent = Arc::new(Agent::new(config.clone(), provider, tools).with_model(model));

        let handle = AgentHandle {
            id: id.clone(),
            config: config.clone(),
            tx: tx.clone(),
            busy: false,
            agent: agent.clone(),
        };

        {
            let mut agents = self.state.agents.write().await;
            agents.insert(id.clone(), handle);
        }

        // Start agent processing loop
        let state = self.state.clone();
        let agent_id = id.clone();

        tokio::spawn(async move {
            info!("Agent {} processing loop started", agent_id);

            while let Some(cmd) = rx.recv().await {
                match cmd {
                    AgentCommand::ProcessMessage {
                        session_id,
                        message,
                        user_id,
                        channel: _,
                    } => {
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

                        // Create progress callback that broadcasts tool events
                        let progress_state = state.clone();
                        let progress_session_id = session_id.clone();
                        let progress_agent_id = agent_id.clone();
                        let progress_cb: crate::agent::ProgressCallback =
                            Arc::new(move |event: crate::agent::ProgressEvent| {
                                let state = progress_state.clone();
                                let session_id = progress_session_id.clone();
                                let agent_id = progress_agent_id.clone();
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
                                                    tool_name: name,
                                                    arguments: arguments.clone(),
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
                                                tool_name: name,
                                                result: result.clone(),
                                            });
                                        }
                                        _ => {} // Ignore other events for now
                                    }
                                })
                            });

                        // Process message with progress callbacks
                        let response_content = match agent
                            .process_message_with_progress(incoming_msg, progress_cb)
                            .await
                        {
                            Ok(outgoing) => outgoing.content,
                            Err(e) => {
                                error!("Agent {} failed to process message: {}", agent_id, e);
                                format!("Error processing message: {}", e)
                            }
                        };

                        // Send response event
                        let _ = state.event_tx.send(GatewayEvent::AgentResponse {
                            session_id: session_id.clone(),
                            agent_id: agent_id.clone(),
                            content: response_content,
                        });

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
            }

            info!("Agent {} processing loop ended", agent_id);
        });

        Ok(())
    }

    /// Initialize configured channels
    async fn init_channels(&self) -> crate::Result<()> {
        info!("Initializing {} configured channels", self.config.channels.len());

        for (name, config) in &self.config.channels {
            if !config.enabled {
                info!("Channel {} is disabled, skipping", name);
                continue;
            }

            info!("Initializing channel {} ({:?})", name, config.channel_type);

            match config.channel_type {
                ChannelType::Telegram => {
                    #[cfg(feature = "telegram")]
                    {
                        if let Some(token) = config.credentials.get("token") {
                            let telegram_config =
                                crate::channels::telegram::TelegramConfig::new(token)
                                    .allow_usernames(config.allow_from.clone());

                            let channel = Arc::new(
                                crate::channels::telegram::TelegramChannel::new(telegram_config),
                            );

                            // Create message channel for routing Telegram -> Gateway agent
                            let (message_tx, mut message_rx) = mpsc::unbounded_channel::<crate::channels::IncomingMessage>();

                            // Set up the global message queue sender for Telegram channel
                            crate::channels::telegram::set_message_queue_sender(message_tx);

                            // Get default agent for routing messages (clone the handle)
                            let agent_handle = self.state.agents.read().await.get("default").cloned();
                            if agent_handle.is_none() {
                                warn!("No default agent available for Telegram channel '{}'", name);
                            }

                            // Spawn task to process messages from Telegram and route to agent
                            let agent_for_task = agent_handle.clone();
                            tokio::spawn(async move {
                                while let Some(message) = message_rx.recv().await {
                                    if let Some(ref handle) = agent_for_task {
                                        // Send to agent via its command channel
                                        let cmd = AgentCommand::ProcessMessage {
                                            session_id: message.conversation_id.0.clone(),
                                            message: message.content.clone(),
                                            user_id: message.user_id.0.clone(),
                                            channel: "telegram".to_string(),
                                        };
                                        if let Err(e) = handle.tx.send(cmd).await {
                                            error!("Failed to send message to agent: {}", e);
                                        }
                                    } else {
                                        error!("No default agent available to process Telegram message");
                                    }
                                }
                            });

                            // Subscribe to agent responses and send back to Telegram
                            let mut event_rx = self.state.event_tx.subscribe();
                            let channel_for_telegram = channel.clone();
                            let channel_name = name.clone();
                            tokio::spawn(async move {
                                loop {
                                    match event_rx.recv().await {
                                        Ok(GatewayEvent::AgentResponse { session_id, content, .. }) => {
                                            // Send response back to Telegram
                                            let conversation_id = crate::channels::ConversationId::new(session_id.clone());
                                            let outgoing = crate::channels::OutgoingMessage::new(
                                                conversation_id,
                                                content.clone()
                                            );
                                            if let Err(e) = channel_for_telegram.send(outgoing).await {
                                                error!("Failed to send response to Telegram channel '{}': {}", channel_name, e);
                                            } else {
                                                info!("Sent agent response to Telegram session {}", session_id);
                                            }
                                        }
                                        Err(_) => {
                                            // Event channel closed or lagged, break loop
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                            });

                            let channel_name = name.clone();
                            // Start the channel in a background task
                            let channel_for_task = channel.clone();
                            tokio::spawn(async move {
                                if let Err(e) = channel_for_task.start().await {
                                    error!("Telegram channel {} failed: {}", channel_name, e);
                                }
                            });
                            self.state
                                .channels
                                .write()
                                .await
                                .insert(name.clone(), channel);
                            info!("✅ Telegram channel '{}' initialized", name);
                        } else {
                            warn!("Telegram channel '{}' missing 'token' in credentials", name);
                        }
                    }
                    #[cfg(not(feature = "telegram"))]
                    {
                        warn!("Telegram feature not enabled, skipping channel '{}'", name);
                    }
                }
                ChannelType::Discord => {
                    #[cfg(feature = "discord")]
                    {
                        if let Some(token) = config.credentials.get("token") {
                            let discord_config =
                                crate::channels::discord::DiscordConfig::new(token);

                            let channel = Arc::new(crate::channels::discord::DiscordChannel::new(
                                discord_config,
                            ));
                            let channel_name = name.clone();
                            let channel_for_task = channel.clone();
                            tokio::spawn(async move {
                                if let Err(e) = channel_for_task.start().await {
                                    error!("Discord channel {} failed: {}", channel_name, e);
                                }
                            });
                            self.state
                                .channels
                                .write()
                                .await
                                .insert(name.clone(), channel);
                            info!("✅ Discord channel '{}' initialized", name);
                        } else {
                            warn!("Discord channel '{}' missing 'token' in credentials", name);
                        }
                    }
                    #[cfg(not(feature = "discord"))]
                    {
                        warn!("Discord feature not enabled, skipping channel '{}'", name);
                    }
                }
                ChannelType::Slack => {
                    #[cfg(feature = "slack")]
                    {
                        if let Some(token) = config.credentials.get("token") {
                            let slack_config = crate::channels::slack::SlackConfig::new(token);

                            let channel =
                                Arc::new(crate::channels::slack::SlackChannel::new(slack_config));
                            let channel_name = name.clone();
                            let channel_for_task = channel.clone();
                            tokio::spawn(async move {
                                if let Err(e) = channel_for_task.start().await {
                                    error!("Slack channel {} failed: {}", channel_name, e);
                                }
                            });
                            self.state
                                .channels
                                .write()
                                .await
                                .insert(name.clone(), channel);
                            info!("✅ Slack channel '{}' initialized", name);
                        } else {
                            warn!("Slack channel '{}' missing 'token' in credentials", name);
                        }
                    }
                    #[cfg(not(feature = "slack"))]
                    {
                        warn!("Slack feature not enabled, skipping channel '{}'", name);
                    }
                }
                ChannelType::Whatsapp => {
                    #[cfg(feature = "whatsapp")]
                    {
                        // WhatsApp requires both phone_number_id and access_token
                        if let (Some(phone_id), Some(token)) = (
                            config.credentials.get("phone_number_id"),
                            config.credentials.get("access_token"),
                        ) {
                            let whatsapp_config =
                                crate::channels::whatsapp::WhatsappConfig::new(phone_id, token);

                            let channel = Arc::new(
                                crate::channels::whatsapp::WhatsappChannel::new(whatsapp_config),
                            );
                            let channel_name = name.clone();
                            let channel_for_task = channel.clone();
                            tokio::spawn(async move {
                                if let Err(e) = channel_for_task.start().await {
                                    error!("WhatsApp channel {} failed: {}", channel_name, e);
                                }
                            });
                            self.state
                                .channels
                                .write()
                                .await
                                .insert(name.clone(), channel);
                            info!("✅ WhatsApp channel '{}' initialized", name);
                        } else {
                            warn!("WhatsApp channel '{}' missing 'phone_number_id' or 'access_token' in credentials", name);
                        }
                    }
                    #[cfg(not(feature = "whatsapp"))]
                    {
                        warn!("WhatsApp feature not enabled, skipping channel '{}'", name);
                    }
                }
                ChannelType::Qq => {
                    #[cfg(feature = "qq")]
                    {
                        // QQ requires app_id, app_secret, and bot_qq
                        if let (Some(app_id), Some(app_secret), Some(bot_qq)) = (
                            config.credentials.get("app_id"),
                            config.credentials.get("app_secret"),
                            config.credentials.get("bot_qq"),
                        ) {
                            let qq_config =
                                crate::channels::qq::QqConfig::new(app_id, app_secret, bot_qq);

                            let channel = Arc::new(crate::channels::qq::QqChannel::new(qq_config));
                            let channel_name = name.clone();
                            let channel_for_task = channel.clone();
                            tokio::spawn(async move {
                                if let Err(e) = channel_for_task.start().await {
                                    error!("QQ channel {} failed: {}", channel_name, e);
                                }
                            });
                            self.state
                                .channels
                                .write()
                                .await
                                .insert(name.clone(), channel);
                            info!("✅ QQ channel '{}' initialized", name);
                        } else {
                            warn!("QQ channel '{}' missing required credentials (app_id, app_secret, bot_qq)", name);
                        }
                    }
                    #[cfg(not(feature = "qq"))]
                    {
                        warn!("QQ feature not enabled, skipping channel '{}'", name);
                    }
                }
                ChannelType::Feishu | ChannelType::WebTerminal => {
                    // Feishu/Lark and WebTerminal are handled via webhooks/SocketMode
                    // They don't need a persistent connection here
                    info!(
                        "Channel '{}' ({:?}) uses webhook/SocketMode, skipping adapter spawn",
                        name, config.channel_type
                    );
                }
                ChannelType::Websocket => {
                    info!("WebSocket channel '{}' requires external connection", name);
                }
            }
        }

        let channel_count = self.state.channels.read().await.len();
        info!("✅ Initialized {} active channel connections", channel_count);

        Ok(())
    }

    /// Process message queue
    async fn process_message_queue(
        state: Arc<GatewayState>,
        mut rx: mpsc::Receiver<QueuedMessage>,
    ) {
        while let Some(msg) = rx.recv().await {
            info!("Processing queued message: {}", msg.id);

            // Route to appropriate agent
            let agent_id = Self::resolve_agent_for_session(&state, &msg.session_id).await;

            let agents = state.agents.read().await;
            if let Some(agent) = agents.get(&agent_id) {
                let cmd = AgentCommand::ProcessMessage {
                    session_id: msg.session_id,
                    message: msg.content,
                    user_id: msg.user_id,
                    channel: msg.channel,
                };

                if let Err(e) = agent.tx.send(cmd).await {
                    error!("Failed to send command to agent {}: {}", agent_id, e);
                }
            } else {
                error!("Agent {} not found for session {}", agent_id, msg.id);
            }
        }
    }

    /// Resolve which agent should handle a session
    async fn resolve_agent_for_session(state: &Arc<GatewayState>, session_id: &str) -> String {
        let routing = state.session_routing.read().await;
        routing
            .get(session_id)
            .cloned()
            .unwrap_or_else(|| "default".to_string())
    }

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

    /// Start web terminal server
    async fn start_web_terminal(port: u16, state: Arc<GatewayState>) -> crate::Result<()> {
        info!("Web terminal starting on port {}", port);

        // Build web terminal router with state
        let app = Router::new()
            .route("/", get(web_terminal_html_handler))
            .route("/ws", get(web_terminal_ws_handler))
            .with_state(state);

        let addr = format!("127.0.0.1:{}", port);
        let listener = TcpListener::bind(&addr).await.map_err(|e| {
            crate::error::MantaError::ExternalService {
                source: "Failed to bind web terminal".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;

        info!("Web Terminal available at http://{}", addr);

        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                error!("Web terminal server error: {}", e);
            }
        });

        Ok(())
    }

    /// Register hot reload handlers for config changes
    async fn register_hot_reload_handlers(&self, hot_reload: &HotReloadManager) {
        use crate::config::hot_reload::ConfigFileType;

        // Handler for main config changes
        hot_reload
            .register_handler(ConfigFileType::Main, |_event| async move {
                info!("Main config file changed - reloading configuration");
                // In a full implementation, this would:
                // 1. Reload the config file
                // 2. Update GatewayConfig
                // 3. Notify components of changes
                // 4. Possibly restart affected services
                Ok(())
            })
            .await;

        // Handler for agent config changes
        hot_reload
            .register_handler(ConfigFileType::Agent, |event| async move {
                info!("Agent config changed: {:?} - reloading agent settings", event.path);
                // Would update agent configurations dynamically
                Ok(())
            })
            .await;

        // Handler for channel config changes
        hot_reload
            .register_handler(ConfigFileType::Channel, |event| async move {
                info!("Channel config changed: {:?} - updating channel settings", event.path);
                // Would update channel configurations dynamically
                Ok(())
            })
            .await;

        // Handler for plugin config changes
        hot_reload
            .register_handler(ConfigFileType::Plugin, |event| async move {
                info!("Plugin config changed: {:?} - reloading plugins", event.path);
                // Would reload plugin configurations
                Ok(())
            })
            .await;

        // Handler for gateway config changes
        hot_reload
            .register_handler(ConfigFileType::Gateway, |event| async move {
                info!("Gateway config changed: {:?} - updating gateway settings", event.path);
                // Would update gateway routes, security settings, etc.
                Ok(())
            })
            .await;

        info!("Registered hot reload handlers for all config types");
    }
}

/// HTML handler for web terminal
async fn web_terminal_html_handler() -> Html<String> {
    Html(include_str!("../../assets/web_terminal.html").replace("{VERSION}", crate::VERSION))
}

/// WebSocket upgrade handler for web terminal
async fn web_terminal_ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_web_terminal_websocket(socket, query, state))
}

/// Handle WebSocket connection for web terminal
async fn handle_web_terminal_websocket(
    mut socket: axum::extract::ws::WebSocket,
    query: WsQuery,
    state: Arc<GatewayState>,
) {
    use axum::extract::ws::Message;

    // Generate session ID
    let session_id = if query.new == Some(true) {
        uuid::Uuid::new_v4().to_string()
    } else if let Some(conv) = query.conversation {
        conv
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    info!("Web terminal connected, session: {}", session_id);

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "system",
        "content": format!("Connected to Manta Gateway.\nSession: {}\nType /new to start a fresh conversation.", session_id)
    });
    if let Err(e) = socket.send(Message::Text(welcome.to_string())).await {
        error!("Failed to send welcome: {}", e);
        return;
    }

    // Create event subscription for this session
    let mut event_rx = state.event_tx.subscribe();

    // Main message loop - use select! to handle both incoming messages and events
    loop {
        tokio::select! {
            // Handle incoming WebSocket messages from client
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Handle /new command
                        if text.trim() == "/new" {
                            let new_id = uuid::Uuid::new_v4().to_string();
                            let system_msg = serde_json::json!({
                                "type": "system",
                                "content": format!("🆕 Started new session: {}", new_id)
                            });
                            if socket.send(Message::Text(system_msg.to_string())).await.is_err() {
                                break;
                            }
                            continue;
                        }

                        // Send typing indicator
                        let typing = serde_json::json!({
                            "type": "typing",
                            "content": true
                        });
                        if socket.send(Message::Text(typing.to_string())).await.is_err() {
                            break;
                        }

                        // Route message to AI agent via GatewayState
                        let agents = state.agents.read().await;
                        if let Some(agent_handle) = agents.get("default") {
                            // Send ProcessMessage command to agent
                            let cmd = AgentCommand::ProcessMessage {
                                session_id: session_id.clone(),
                                message: text.clone(),
                                user_id: "web_user".to_string(),
                                channel: "web".to_string(),
                            };

                            if let Err(e) = agent_handle.tx.send(cmd).await {
                                error!("Failed to send message to agent: {}", e);
                                let error_msg = serde_json::json!({
                                    "type": "error",
                                    "content": "Failed to process message"
                                });
                                let _ = socket.send(Message::Text(error_msg.to_string())).await;
                            }
                            // Don't wait for response here - it will come through events
                        } else {
                            // No default agent available
                            let error_msg = serde_json::json!({
                                "type": "error",
                                "content": "No AI agent available"
                            });
                            if socket.send(Message::Text(error_msg.to_string())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Ok(Message::Binary(_))) => {
                        info!("Web terminal disconnected");
                        break;
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            // Handle gateway events (tool calls, responses, etc.)
            event = event_rx.recv() => {
                match event {
                    Ok(GatewayEvent::ToolCalling { session_id: event_session, tool_name, arguments, .. }) => {
                        info!("WebSocket received ToolCalling event for session {}: {}", event_session, tool_name);
                        if event_session == session_id {
                            let tool_msg = serde_json::json!({
                                "type": "tool_call",
                                "tool": tool_name,
                                "arguments": arguments
                            });
                            let msg_text = tool_msg.to_string();
                            info!("Sending tool_call message to WebSocket: {}", msg_text);
                            if socket.send(Message::Text(msg_text)).await.is_err() {
                                break;
                            }
                        } else {
                            tracing::debug!("ToolCalling event for different session: {} vs {}", event_session, session_id);
                        }
                    }
                    Ok(GatewayEvent::ToolResult { session_id: event_session, tool_name, result, .. }) => {
                        info!("WebSocket received ToolResult event for session {}: {}", event_session, tool_name);
                        if event_session == session_id {
                            let result_msg = serde_json::json!({
                                "type": "tool_result",
                                "tool": tool_name,
                                "result": result
                            });
                            let msg_text = result_msg.to_string();
                            info!("Sending tool_result message to WebSocket: {}", msg_text);
                            if socket.send(Message::Text(msg_text)).await.is_err() {
                                break;
                            }
                        } else {
                            tracing::debug!("ToolResult event for different session: {} vs {}", event_session, session_id);
                        }
                    }
                    Ok(GatewayEvent::AgentResponse { session_id: resp_session, content, .. }) => {
                        if resp_session == session_id {
                            // Turn off typing indicator
                            let typing_off = serde_json::json!({
                                "type": "typing",
                                "content": false
                            });
                            let _ = socket.send(Message::Text(typing_off.to_string())).await;

                            // Send AI response
                            let response = serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": content
                            });
                            if socket.send(Message::Text(response.to_string())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(_) => {
                        // Other events - ignore for now
                    }
                    Err(_) => {
                        // Event channel closed or lagged
                        continue;
                    }
                }
            }
        }
    }
}

/// Create default tool registry with all built-in tools
async fn create_default_tool_registry(acp: Arc<AcpControlPlane>) -> crate::Result<ToolRegistry> {
    use crate::tools::*;

    let mut registry = ToolRegistry::new();

    // Register file system tools
    registry.register(Box::new(FileReadTool::new()));
    registry.register(Box::new(FileWriteTool::new()));
    registry.register(Box::new(FileEditTool::new()));
    registry.register(Box::new(GlobTool::new()));
    registry.register(Box::new(GrepTool::new()));

    // Register shell/execution tools
    registry.register(Box::new(ShellTool::new()));
    registry.register(Box::new(CodeExecutionTool::default()));

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
    registry.register(Box::new(AcpSpawnTool::new(acp.clone())));
    registry.register(Box::new(AcpSessionTool::new(acp.clone())));

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

    // Register delegation tool for agent-to-agent task delegation
    registry.register(Box::new(DelegateTool::root()));

    // Register MCP (Model Context Protocol) connection tool
    registry.register(Box::new(McpConnectionTool::new()));

    Ok(registry)
}

// HTTP Handlers
async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "version": crate::VERSION,
        "agent": "ready",
    }))
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

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, state))
}

async fn handle_websocket(socket: axum::extract::ws::WebSocket, state: Arc<GatewayState>) {
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};

    info!("Gateway events WebSocket connected");

    // Subscribe to gateway events
    let mut event_rx = state.event_tx.subscribe();

    // Split socket for send/receive
    let (mut sender, mut receiver) = socket.split();

    // Task to receive gateway events and send to client
    let event_task = tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            let msg = Message::Text(serde_json::to_string(&event).unwrap_or_default());
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Task to receive client messages (ping/pong, commands)
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Close(_) => break,
                Message::Ping(data) => {
                    // Pong is handled automatically by axum
                    debug!("Received ping: {:?}", data);
                }
                Message::Text(text) => {
                    // Client can send commands (optional)
                    debug!("Received WebSocket message: {}", text);
                }
                _ => {}
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = event_task => {}
        _ = recv_task => {}
    }

    info!("Gateway events WebSocket disconnected");
}

async fn list_agents_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let agents = state.agents.read().await;
    let list: Vec<_> = agents
        .iter()
        .map(|(id, handle)| {
            serde_json::json!({
                "id": id,
                "busy": handle.busy,
            })
        })
        .collect();
    Json(list)
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

    // Get tools and model
    let tools = state.tool_registry.clone();
    let model = state.config.read().await.model.clone();

    // Create agent instance
    let agent = Arc::new(Agent::new(config.clone(), provider, tools).with_model(model));

    // Create agent handle
    let handle = AgentHandle {
        id: agent_id.clone(),
        config: config.clone(),
        tx: tx.clone(),
        busy: false,
        agent: agent.clone(),
    };

    // Insert into agents map
    {
        let mut agents = state.agents.write().await;
        agents.insert(agent_id.clone(), handle);
    }

    // Start agent processing loop
    let state_clone = state.clone();
    let agent_id_clone = agent_id.clone();
    tokio::spawn(async move {
        info!("Agent {} processing loop started", agent_id_clone);
        while let Some(cmd) = rx.recv().await {
            match cmd {
                AgentCommand::Shutdown => {
                    info!("Agent {} shutting down", agent_id_clone);
                    let _ = state_clone.event_tx.send(GatewayEvent::AgentStatus {
                        agent_id: agent_id_clone.clone(),
                        status: AgentStatus::Shutdown,
                    });
                    break;
                }
                _ => {
                    // Handle other commands (simplified for API-created agents)
                    info!("Agent {} received command: {:?}", agent_id_clone, cmd);
                }
            }
        }
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

async fn send_message_handler(
    State(state): State<Arc<GatewayState>>,
    Path(session_id): Path<String>,
    Json(body): Json<SendMessageRequest>,
) -> impl IntoResponse {
    // Check if provider override is specified
    let provider_override = body.provider_override.clone();
    let _model_alias = body.model_alias.clone();

    // Queue message for processing with provider override
    let message_id = uuid::Uuid::new_v4().to_string();

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

    // Otherwise, queue for normal agent processing
    // TODO: Implement proper message queue processing with model_alias support
    let queued_msg = QueuedMessage {
        id: message_id.clone(),
        channel: "api".to_string(),
        user_id: "api_user".to_string(),
        content: body.message,
        session_id: session_id.clone(),
        timestamp: chrono::Utc::now(),
    };

    if let Err(e) = state.message_queue.send(queued_msg).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("Failed to queue message: {}", e),
            })),
        )
            .into_response();
    }

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
    let (event_tx, mut event_rx) = mpsc::channel::<CanvasEvent>(100);

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

    // Task to receive client events
    let event_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(text) = msg {
                if let Ok(event) = serde_json::from_str::<CanvasEvent>(&text) {
                    let _ = event_rx.recv().await;
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

async fn list_models_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let aliases = state.model_router.list_aliases().await;
    Json(serde_json::json!({
        "aliases": aliases,
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

    // Check if skill exists and is eligible
    match skills_manager.get_skill(&id).await {
        Some(skill) => {
            if !skill.enabled {
                let error = serde_json::json!({
                    "error": format!("Skill '{}' is disabled", id),
                });
                return (StatusCode::BAD_REQUEST, Json(error)).into_response();
            }

            if !skill.is_eligible {
                let error = serde_json::json!({
                    "error": format!("Skill '{}' is not eligible to run", id),
                    "reasons": skill.eligibility_errors,
                });
                return (StatusCode::BAD_REQUEST, Json(error)).into_response();
            }

            // Return the skill prompt that would be used
            // In a full implementation, this would queue the skill for execution by an agent
            let response = serde_json::json!({
                "skill_id": id,
                "status": "ready",
                "prompt": skill.prompt,
                "input": body.input,
                "context": body.context.unwrap_or(serde_json::json!({})),
                "note": "Skill is ready for execution. Send this prompt to an agent to execute.",
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": format!("Skill '{}' not found", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
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

async fn spawn_subagent_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SpawnSubagentRequest>,
) -> impl IntoResponse {
    use crate::acp::{AcpSessionId, SpawnMode, SubagentConfig, ThreadBinding};
    use crate::channels::IncomingMessage;

    let session_id = AcpSessionId::new();
    let parent_id = "gateway-api".to_string();

    let mode = match body.mode.as_str() {
        "session" => SpawnMode::Session,
        _ => SpawnMode::Run,
    };

    let config = SubagentConfig {
        agent_type: if body.agent_type.is_empty() {
            "default".to_string()
        } else {
            body.agent_type
        },
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
        .spawn_subagent(session_id.clone(), parent_id, config)
        .await
    {
        Ok(handle) => {
            let subagent_id = handle.id.clone();

            // Send task to subagent
            let message =
                IncomingMessage::new("api-user".to_string(), session_id.to_string(), body.task);

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

    let session_id = AcpSessionId(id);
    match state.acp.terminate_session(&session_id).await {
        Ok(count) => {
            let response = serde_json::json!({
                "terminated_count": count,
                "session_id": session_id.to_string(),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
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

    // Find a subagent in this session
    let session_id = AcpSessionId(id);
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
            let resp = serde_json::json!({
                "subagent_id": subagent.id,
                "session_id": session_id.to_string(),
                "response": response,
            });
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to send message: {}", e),
            });
            (StatusCode::INTERNAL_SERVER_ERROR, Json(error)).into_response()
        }
    }
}
