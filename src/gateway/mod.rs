//! Gateway Control Plane
//!
//! The Gateway is the control plane for Manta, managing:
//! - Multi-channel message routing (WhatsApp, Telegram, Feishu, etc.)
//! - Session management and routing to agents
//! - Agent spawning and lifecycle management
//! - WebSocket/HTTP API for channel adapters
//! - Authentication and security policies

use axum::{
    extract::{Path, State, WebSocketUpgrade, Query, ConnectInfo},
    http::StatusCode,
    middleware::from_fn,
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{SocketAddr, IpAddr};
use std::sync::Arc;
use tokio::net::TcpListener;
use futures_util::{StreamExt, SinkExt};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::{Agent, AgentConfig};
use crate::canvas::{CanvasEvent, CanvasManager};
use crate::channels::{Channel, ChannelType};
use crate::config::hot_reload::{HotReloadManager, ConfigFileType};
use crate::memory::vector::{VectorMemoryService, ApiEmbeddingProvider, EmbeddingConfig, MemoryVectorStore};
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

/// Vector memory configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMemoryConfig {
    /// Enable vector memory / semantic search
    pub enabled: bool,
    /// Embedding provider API key (e.g., OpenAI)
    pub embedding_api_key: Option<String>,
    /// Embedding model to use
    pub embedding_model: String,
    /// Embedding dimension
    pub embedding_dimension: usize,
    /// API base URL (for Azure, etc.)
    pub api_base_url: Option<String>,
}

impl Default for VectorMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            embedding_api_key: None,
            embedding_model: "text-embedding-3-small".to_string(),
            embedding_dimension: 1536,
            api_base_url: None,
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
    pub channels: Arc<RwLock<HashMap<String, Box<dyn Channel>>>>,
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
}

/// Handle to a running agent
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
    ChannelStatus {
        channel: String,
        connected: bool,
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
        let tool_registry = Arc::new(create_default_tool_registry(acp.clone())?);

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
        });

        // Configure providers from config
        for (name, provider_config) in &config.providers {
            info!("Configuring provider: {}", name);
            if let Err(e) = state.model_router.add_provider(name, provider_config.clone()).await {
                warn!("Failed to add provider '{}': {}", name, e);
            }
        }

        // Initialize vector memory service if enabled
        if config.vector_memory.enabled {
            info!("Initializing vector memory service...");

            if let Some(ref api_key) = config.vector_memory.embedding_api_key {
                // Create API embedding provider with explicit parameters
                let mut provider = ApiEmbeddingProvider::new(
                    api_key.clone(),
                    config.vector_memory.embedding_model.clone(),
                    config.vector_memory.embedding_dimension,
                );

                // Set custom base URL if provided
                if let Some(ref base_url) = config.vector_memory.api_base_url {
                    provider = provider.with_base_url(base_url.clone());
                }

                let embedding_provider: Arc<dyn crate::memory::vector::EmbeddingProvider> = Arc::new(provider);
                let vector_store = Arc::new(MemoryVectorStore::new(config.vector_memory.embedding_dimension));

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
                info!("Vector memory service initialized");
                *state.vector_memory.write().await = Some(service);
            } else {
                warn!("Vector memory enabled but no API key provided");
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

        // Start message processing worker
        tokio::spawn(Self::process_message_queue(
            state.clone(),
            message_queue_rx,
        ));

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

        // Initialize hot reload if enabled
        let hot_reload = self.state.hot_reload.read().await.clone();
        if let Some(ref hot_reload) = hot_reload {
            let config_path = crate::dirs::default_config_file();
            if let Err(e) = hot_reload.watch_file(&config_path, ConfigFileType::Main).await {
                warn!("Failed to watch config file: {}", e);
            }
            // Start hot reload processing in background
            let hot_reload_clone = hot_reload.clone();
            tokio::spawn(async move {
                if let Err(e) = hot_reload_clone.run().await {
                    error!("Hot reload error: {}", e);
                }
            });
        }

        // Initialize default agent (optional - requires provider configuration)
        match self.spawn_agent("default".to_string(), self.config.default_agent.clone()).await {
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
        tokio::spawn(Self::start_web_terminal(
            self.config.web_port,
            self.state.clone(),
        ));

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
            // ACP (Agent Control Plane) API
            .route("/api/v1/acp/sessions", get(list_acp_sessions_handler))
            .route("/api/v1/acp/sessions", post(spawn_subagent_handler))
            .route("/api/v1/acp/sessions/:id", delete(terminate_acp_session_handler))
            .route("/api/v1/acp/sessions/:id/message", post(acp_session_message_handler))
            // Apply localhost/Tailscale restriction middleware
            .layer(from_fn(middleware::tailscale_only_middleware))
            .with_state(state.clone());

        // Merge public and admin routers
        public_router.merge(admin_router)
    }

    /// Spawn a new agent
    async fn spawn_agent(&self, id: String, config: AgentConfig) -> crate::Result<()> {
        info!("Spawning agent: {}", id);

        let (tx, mut rx) = mpsc::channel(100);

        // Create provider from model router
        let provider: Arc<dyn crate::providers::Provider> = self.state.model_router.create_default_provider().await?;

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
                        info!(
                            "Agent {} processing message for session {}",
                            agent_id, session_id
                        );

                        // Update status to processing
                        let _ = state.event_tx.send(GatewayEvent::AgentStatus {
                            agent_id: agent_id.clone(),
                            status: AgentStatus::Processing {
                                session_id: session_id.clone(),
                            },
                        });

                        // Create incoming message for the Agent
                        let incoming_msg = crate::channels::IncomingMessage::new(
                            user_id.clone(),
                            session_id.clone(),
                            message.clone(),
                        );

                        // Process message through actual Agent runtime
                        let response_content = match agent.process_message(incoming_msg).await {
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
                        // TODO: Update agent configuration dynamically
                        let _ = new_config;
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

        // Channels are now initialized separately via their own adapters
        // Each adapter connects to the Gateway via WebSocket
        for (name, config) in &self.config.channels {
            if !config.enabled {
                info!("Channel {} is disabled, skipping", name);
                continue;
            }

            info!("Channel {} ({:?}) will be initialized by adapter", name, config.channel_type);
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
    async fn resolve_agent_for_session(
        state: &Arc<GatewayState>,
        session_id: &str,
    ) -> String {
        let routing = state.session_routing.read().await;
        routing.get(session_id).cloned().unwrap_or_else(|| "default".to_string())
    }

    /// Start Tailscale for remote access
    async fn start_tailscale(&self) -> crate::Result<()> {
        #[cfg(feature = "tailscale")]
        {
            info!("Starting Tailscale integration...");
            crate::tailscale::start(
                self.config.port,
                self.config.tailscale_domain.clone(),
            ).await?;
        }

        #[cfg(not(feature = "tailscale"))]
        {
            warn!("Tailscale feature not compiled in. Install with: cargo build --features tailscale");
        }

        Ok(())
    }

    /// Start web terminal server
    async fn start_web_terminal(port: u16, _state: Arc<GatewayState>) -> crate::Result<()> {
        info!("Web terminal starting on port {}", port);

        // Build web terminal router
        let app = Router::new()
            .route("/", get(web_terminal_html_handler))
            .route("/ws", get(web_terminal_ws_handler));

        let addr = format!("0.0.0.0:{}", port);
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
}

/// HTML handler for web terminal
async fn web_terminal_html_handler() -> Html<String> {
    Html(include_str!("../../assets/web_terminal.html").replace("{VERSION}", crate::VERSION))
}

/// WebSocket upgrade handler for web terminal
async fn web_terminal_ws_handler(
    ws: WebSocketUpgrade,
    Query(query): Query<WsQuery>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_web_terminal_websocket(socket, query))
}

/// Handle WebSocket connection for web terminal
async fn handle_web_terminal_websocket(
    mut socket: axum::extract::ws::WebSocket,
    query: WsQuery,
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

    // Main message loop
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
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

                // NOTE: The web terminal WebSocket currently echoes messages back.
                // To fully integrate with agents, this needs access to GatewayState.
                // For now, we provide a helpful message directing users to use the /chat endpoint.
                let response = serde_json::json!({
                    "type": "message",
                    "role": "assistant",
                    "content": format!("Echo: {}\n\n(Session: {})\n\nNote: Full AI integration via WebSocket requires Gateway state access. Use POST /chat API for AI responses.", text, session_id)
                });

                if socket.send(Message::Text(response.to_string())).await.is_err() {
                    break;
                }

                // Turn off typing indicator
                let typing_off = serde_json::json!({
                    "type": "typing",
                    "content": false
                });
                if socket.send(Message::Text(typing_off.to_string())).await.is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) | Ok(Message::Binary(_)) => {
                info!("Web terminal disconnected");
                break;
            }
            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }
}

/// Create default tool registry with all built-in tools
fn create_default_tool_registry(acp: Arc<AcpControlPlane>) -> crate::Result<ToolRegistry> {
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

    Ok(registry)
}

// HTTP Handlers
async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "version": crate::VERSION,
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
    let conversation_id = body.conversation_id.unwrap_or_else(|| "default".to_string());

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
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                "error": format!("Failed to send message to agent: {}", e),
            })));
        }

        // Drop the agents lock so we don't hold it while waiting
        drop(agents);

        // Wait for response with timeout
        let timeout = tokio::time::Duration::from_secs(120);
        let start = tokio::time::Instant::now();

        loop {
            // Check for timeout
            if start.elapsed() > timeout {
                return (StatusCode::REQUEST_TIMEOUT, Json(serde_json::json!({
                    "error": "Request timeout",
                })));
            }

            // Wait for event with a smaller timeout to allow checking
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                event_rx.recv()
            ).await {
                Ok(Ok(GatewayEvent::AgentResponse { session_id, agent_id: _, content })) => {
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
                    return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
                        "error": "Event channel closed",
                    })));
                }
                Err(_) => {
                    // Timeout on recv, continue loop to check overall timeout
                    continue;
                }
            }
        }
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
            "error": "No default agent available",
        })))
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, state))
}

async fn handle_websocket(
    socket: axum::extract::ws::WebSocket,
    state: Arc<GatewayState>,
) {
    // WebSocket handling for real-time events
    // TODO: Implement full WebSocket protocol
}

async fn list_agents_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
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
    // Create new agent
    // TODO: Implement
    (StatusCode::CREATED, Json(serde_json::json!({"id": "new-agent"})))
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
        })).into_response(),
        None => (StatusCode::NOT_FOUND, "Agent not found").into_response(),
    }
}

async fn delete_agent_handler(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    // Shutdown and remove agent
    // TODO: Implement
    (StatusCode::NO_CONTENT, ())
}

async fn list_channels_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
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
        match state.model_router.complete_with_provider(
            &provider_name,
            body.model_id,
            vec![crate::providers::Message::user(body.message.clone())],
        ).await {
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
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
            "error": format!("Failed to queue message: {}", e),
        }))).into_response();
    }

    let resp = serde_json::json!({
        "message_id": message_id,
        "session_id": session_id,
        "queued": true,
        "status": "processing",
    });
    (StatusCode::ACCEPTED, Json(resp)).into_response()
}

async fn status_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
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
        None => {
            state.canvas_manager.create_session(event_tx).await
        }
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

async fn create_canvas_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
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
    Path(_id): Path<String>,
    State(_state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    // TODO: Return canvas state
    Json(serde_json::json!({
        "canvas_id": _id,
        "status": "active"
    }))
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

async fn list_providers_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
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
    match state.model_router.set_fallback_chain(&alias, body.providers).await {
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

async fn list_models_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let aliases = state.model_router.list_aliases().await;
    Json(serde_json::json!({
        "aliases": aliases,
    }))
}

async fn get_default_model_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
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
            match vm.search_collection(&body.query, body.limit, &body.collection).await {
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
            match vm.add_to_collection(&body.content, body.metadata, &body.collection).await {
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

async fn list_plugins_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
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

// ACP (Agent Control Plane) API Handlers

async fn list_acp_sessions_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
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

    match state.acp.spawn_subagent(session_id.clone(), parent_id, config).await {
        Ok(handle) => {
            let subagent_id = handle.id.clone();

            // Send task to subagent
            let message = IncomingMessage::new(
                "api-user".to_string(),
                session_id.to_string(),
                body.task,
            );

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
    let message = IncomingMessage::new(
        "api-user".to_string(),
        session_id.to_string(),
        body.message,
    );

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
