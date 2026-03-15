//! Gateway Control Plane
//!
//! The Gateway is the control plane for Manta, managing:
//! - Multi-channel message routing (WhatsApp, Telegram, Feishu, etc.)
//! - Session management and routing to agents
//! - Agent spawning and lifecycle management
//! - WebSocket/HTTP API for channel adapters
//! - Authentication and security policies

use axum::{
    extract::{Path, State, WebSocketUpgrade, Query},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use futures_util::{StreamExt, SinkExt};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

use crate::agent::{Agent, AgentConfig};
use crate::canvas::{CanvasEvent, CanvasManager, CanvasWebSocketHandler};
use crate::channels::{Channel, ChannelType};
use crate::model_router::ModelRouter;
use crate::tools::ToolRegistry;

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

        // Create tool registry with built-in tools
        let tool_registry = Arc::new(create_default_tool_registry()?);

        let state = Arc::new(GatewayState {
            config: Arc::new(RwLock::new(config.clone())),
            channels: Arc::new(RwLock::new(HashMap::new())),
            agents: Arc::new(RwLock::new(HashMap::new())),
            session_routing: Arc::new(RwLock::new(HashMap::new())),
            model_router: Arc::new(ModelRouter::default()),
            tool_registry,
            event_tx,
            message_queue: message_queue_tx,
            canvas_manager: Arc::new(CanvasManager::new()),
        });

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

        // Initialize default agent
        self.spawn_agent("default".to_string(), self.config.default_agent.clone())
            .await?;

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
        Router::new()
            .route("/health", get(health_handler))
            .route("/ws", get(ws_handler))
            .route("/ws/canvas/:id", get(canvas_ws_handler))
            .route("/api/v1/agents", get(list_agents_handler).post(create_agent_handler))
            .route(
                "/api/v1/agents/:id",
                get(get_agent_handler).delete(delete_agent_handler),
            )
            .route("/api/v1/channels", get(list_channels_handler))
            .route("/api/v1/sessions/:id/messages", post(send_message_handler))
            .route("/api/v1/status", get(status_handler))
            .route("/api/v1/canvas", post(create_canvas_handler))
            .route("/api/v1/canvas/:id", get(get_canvas_handler).delete(delete_canvas_handler))
            .with_state(self.state.clone())
    }

    /// Spawn a new agent
    async fn spawn_agent(&self, id: String, config: AgentConfig) -> crate::Result<()> {
        info!("Spawning agent: {}", id);

        let (tx, mut rx) = mpsc::channel(100);

        // Create provider from model router
        let provider: Arc<dyn crate::providers::Provider> = self.state.model_router.create_default_provider().await?;

        // Get tool registry from state
        let tools = self.state.tool_registry.clone();

        // Create the actual Agent instance
        let agent = Arc::new(Agent::new(config.clone(), provider, tools));

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

                // TODO: Route message through Gateway to agent
                // For now, echo back with gateway info
                let response = serde_json::json!({
                    "type": "message",
                    "role": "assistant",
                    "content": format!("Received: {}\n(Session: {})", text, session_id)
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
fn create_default_tool_registry() -> crate::Result<ToolRegistry> {
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

    Ok(registry)
}

// HTTP Handlers
async fn health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "healthy",
        "version": crate::VERSION,
    }))
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
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Queue message for processing
    // TODO: Implement
    (StatusCode::ACCEPTED, Json(serde_json::json!({"queued": true})))
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
