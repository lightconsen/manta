//! HTTP Server for Manta
//!
//! Provides REST API endpoints and WebSocket for interacting with the Manta AI assistant.

use crate::core::Engine;
use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, RwLock};
use tracing::{error, info};

/// Global broadcast channel for cron output
static CRON_BROADCAST: RwLock<Option<broadcast::Sender<String>>> = RwLock::const_new(None);

/// Initialize the cron broadcast channel
pub async fn init_cron_broadcast() -> broadcast::Receiver<String> {
    let tx = {
        let guard = CRON_BROADCAST.read().await;
        if let Some(ref tx) = *guard {
            tx.clone()
        } else {
            drop(guard);
            let (tx, _rx) = broadcast::channel(100);
            let mut guard = CRON_BROADCAST.write().await;
            *guard = Some(tx.clone());
            tx
        }
    };
    tx.subscribe()
}

/// Broadcast a cron job output to all connected clients
pub async fn broadcast_cron_output(output: &str) {
    let guard = CRON_BROADCAST.read().await;
    if let Some(ref tx) = *guard {
        // Send as plain text with cron prefix, not JSON
        let msg = format!("📅 {}", output);
        let _ = tx.send(msg);
    }
}

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
    pub agent: Option<Arc<crate::agent::Agent>>,
    pub cron_tx: broadcast::Sender<String>,
}

/// Server configuration
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub web_port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
            web_port: 8080,
        }
    }
}

/// Start the HTTP server with agent and web terminal
pub async fn start_server_with_agent(
    config: ServerConfig,
    engine: Arc<Engine>,
    agent: Arc<crate::agent::Agent>,
) -> crate::Result<()> {
    // Initialize global broadcast channel for cron output
    let cron_tx = {
        let guard = CRON_BROADCAST.read().await;
        if let Some(ref tx) = *guard {
            tx.clone()
        } else {
            drop(guard);
            let (tx, _rx) = broadcast::channel(100);
            let mut guard = CRON_BROADCAST.write().await;
            *guard = Some(tx.clone());
            tx
        }
    };

    let state = AppState {
        engine,
        agent: Some(agent),
        cron_tx: cron_tx.clone(),
    };

    // Start API server
    let api_app = create_api_router(state.clone());
    let api_addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| crate::error::MantaError::Validation(format!("Invalid address: {}", e)))?;

    // Start web terminal server
    let web_app = create_web_router(state);
    let web_addr: SocketAddr = format!("{}:{}", config.host, config.web_port)
        .parse()
        .map_err(|e| crate::error::MantaError::Validation(format!("Invalid address: {}", e)))?;

    info!("Starting API server on {}", api_addr);
    info!("Starting Web Terminal on http://{}", web_addr);
    println!("🌐 Web Terminal available at http://localhost:{}", config.web_port);

    let api_listener = TcpListener::bind(&api_addr)
        .await
        .map_err(|e| crate::error::MantaError::Internal(format!("Failed to bind API: {}", e)))?;

    let web_listener = TcpListener::bind(&web_addr)
        .await
        .map_err(|e| crate::error::MantaError::Internal(format!("Failed to bind Web: {}", e)))?;

    // Run both servers concurrently
    tokio::select! {
        result = axum::serve(api_listener, api_app) => {
            result.map_err(|e| crate::error::MantaError::Internal(format!("API server error: {}", e)))?;
        }
        result = axum::serve(web_listener, web_app) => {
            result.map_err(|e| crate::error::MantaError::Internal(format!("Web server error: {}", e)))?;
        }
    }

    Ok(())
}

/// Create the API router
fn create_api_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/health", get(health_check))
        .route("/chat", post(chat))
        .route("/chat/stream", get(chat_stream))
        .route("/entities", post(create_entity))
        .route("/entities/:id", get(get_entity))
        .route("/entities/:id", post(update_entity))
        .with_state(state)
}

/// Create the web terminal router
fn create_web_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(web_terminal_handler))
        .route("/ws", get(web_socket_handler))
        .with_state(state)
}

/// Root endpoint
async fn root(State(state): State<AppState>) -> impl IntoResponse {
    let agent_status = if state.agent.is_some() {
        "available"
    } else {
        "not configured"
    };

    Json(serde_json::json!({
        "name": "Manta",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "running",
        "agent": agent_status
    }))
}

/// Health check endpoint
async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let agent_status = if state.agent.is_some() { "ready" } else { "disabled" };

    Json(serde_json::json!({
        "status": "healthy",
        "agent": agent_status,
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}

/// Chat request
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    pub conversation_id: Option<String>,
}

/// Chat response
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub response: String,
    pub conversation_id: String,
}

/// Chat endpoint (HTTP)
async fn chat(
    State(state): State<AppState>,
    Json(request): Json<ChatRequest>,
) -> impl IntoResponse {
    if let Some(agent) = &state.agent {
        use crate::channels::IncomingMessage;

        // Use provided conversation ID or get last conversation
        let conversation_id = match request.conversation_id {
            Some(id) => id,
            None => {
                match agent.get_last_conversation("user").await {
                    Ok(Some(last_conv)) => last_conv,
                    _ => crate::channels::ConversationId::generate().to_string(),
                }
            }
        };

        let incoming = IncomingMessage::new("user", &conversation_id, request.message);

        match agent.process_message(incoming).await {
            Ok(response) => {
                let resp = ChatResponse {
                    response: response.content,
                    conversation_id,
                };
                (StatusCode::OK, Json(serde_json::json!(resp)))
            }
            Err(e) => {
                error!("Chat error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            }
        }
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "AI agent not configured"})),
        )
    }
}

/// Chat stream endpoint (WebSocket)
async fn chat_stream(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if state.agent.is_none() {
        return (StatusCode::SERVICE_UNAVAILABLE, "AI agent not configured").into_response();
    }

    ws.on_upgrade(|socket| handle_chat_socket(socket, state))
}

/// Handle WebSocket chat (for CLI and Web)
async fn handle_chat_socket(
    mut socket: axum::extract::ws::WebSocket,
    state: AppState,
) {
    use axum::extract::ws::Message;

    // Subscribe to cron broadcasts
    let mut cron_rx = state.cron_tx.subscribe();

    // Track conversation ID for this connection
    let mut conversation_id: Option<String> = None;

    loop {
        tokio::select! {
            // Handle incoming WebSocket messages
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Try to parse as chat request
                        let request: ChatRequest = match serde_json::from_str(&text) {
                            Ok(r) => r,
                            Err(_) => {
                                // Simple text message - treat as chat
                                ChatRequest {
                                    message: text,
                                    conversation_id: None,
                                }
                            }
                        };

                        if let Some(agent) = &state.agent {
                            use crate::channels::IncomingMessage;

                            // Determine conversation ID
                            let cid = match request.conversation_id {
                                Some(id) => id,
                                None => {
                                    // Use existing conversation for this connection, or get last, or create new
                                    match &conversation_id {
                                        Some(id) => id.clone(),
                                        None => {
                                            // Try to get last conversation
                                            match agent.get_last_conversation("user").await {
                                                Ok(Some(last_conv)) => {
                                                    info!("Resuming last conversation: {}", last_conv);
                                                    last_conv
                                                }
                                                _ => {
                                                    let new_id = crate::channels::ConversationId::generate().to_string();
                                                    info!("Starting new conversation: {}", new_id);
                                                    new_id
                                                }
                                            }
                                        }
                                    }
                                }
                            };

                            // Store for this connection
                            conversation_id = Some(cid.clone());

                            let incoming = IncomingMessage::new("user", &cid, request.message);

                            match agent.process_message(incoming).await {
                                Ok(response) => {
                                    let resp = ChatResponse {
                                        response: response.content,
                                        conversation_id: cid,
                                    };
                                    let _ = socket.send(Message::Text(
                                        serde_json::to_string(&resp).unwrap_or_default()
                                    )).await;
                                }
                                Err(e) => {
                                    let _ = socket.send(Message::Text(
                                        format!("{{\"error\": \"{}\"}}", e)
                                    )).await;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        break;
                    }
                    _ => {}
                }
            }

            // Handle cron broadcasts
            Ok(cron_msg) = cron_rx.recv() => {
                let cron_json = serde_json::json!({
                    "type": "cron",
                    "content": cron_msg
                });
                if socket.send(Message::Text(cron_json.to_string())).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Web terminal HTML page
async fn web_terminal_handler() -> Html<&'static str> {
    Html(TERMINAL_HTML)
}

/// WebSocket handler for browser
async fn web_socket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_chat_socket(socket, state))
}

/// Request to create an entity
#[derive(Debug, Deserialize)]
pub struct CreateEntityRequest {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Entity response
#[derive(Debug, Serialize)]
pub struct EntityResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub tags: Option<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
}

/// Create a new entity
async fn create_entity(
    State(state): State<AppState>,
    Json(request): Json<CreateEntityRequest>,
) -> impl IntoResponse {
    let req = crate::core::models::CreateEntityRequest {
        name: request.name,
        description: request.description,
        tags: request.tags,
    };

    match state.engine.create_entity(req) {
        Ok(entity) => {
            let response = EntityResponse {
                id: entity.id.to_string(),
                name: entity.name,
                description: entity.description,
                status: entity.status.to_string(),
                tags: entity.metadata.tags,
                created_at: entity.metadata.created_at.to_rfc3339(),
                updated_at: entity.metadata.updated_at.to_rfc3339(),
            };
            (StatusCode::CREATED, Json(serde_json::json!(response)))
        }
        Err(e) => {
            error!("Failed to create entity: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        }
    }
}

/// Get an entity by ID
async fn get_entity(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    match crate::core::models::Id::parse(&id) {
        Ok(id) => match state.engine.get_entity(id) {
            Ok(entity) => {
                let response = EntityResponse {
                    id: entity.id.to_string(),
                    name: entity.name,
                    description: entity.description,
                    status: entity.status.to_string(),
                    tags: entity.metadata.tags,
                    created_at: entity.metadata.created_at.to_rfc3339(),
                    updated_at: entity.metadata.updated_at.to_rfc3339(),
                };
                (StatusCode::OK, Json(serde_json::json!(response)))
            }
            Err(e) => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": e.to_string()})),
            ),
        },
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid ID: {}", e)})),
        ),
    }
}

/// Request to update an entity
#[derive(Debug, Deserialize)]
pub struct UpdateEntityRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Update an entity
async fn update_entity(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateEntityRequest>,
) -> impl IntoResponse {
    match crate::core::models::Id::parse(&id) {
        Ok(id) => {
            let status = request.status.and_then(|s| match s.as_str() {
                "active" => Some(crate::core::models::Status::Active),
                "paused" => Some(crate::core::models::Status::Paused),
                "completed" => Some(crate::core::models::Status::Completed),
                "failed" => Some(crate::core::models::Status::Failed),
                _ => None,
            });

            let req = crate::core::models::UpdateEntityRequest {
                name: request.name,
                description: request.description,
                status,
                tags: request.tags,
            };

            match state.engine.update_entity(id, req) {
                Ok(entity) => {
                    let response = EntityResponse {
                        id: entity.id.to_string(),
                        name: entity.name,
                        description: entity.description,
                        status: entity.status.to_string(),
                        tags: entity.metadata.tags,
                        created_at: entity.metadata.created_at.to_rfc3339(),
                        updated_at: entity.metadata.updated_at.to_rfc3339(),
                    };
                    (StatusCode::OK, Json(serde_json::json!(response)))
                }
                Err(e) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": e.to_string()})),
                ),
            }
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("Invalid ID: {}", e)})),
        ),
    }
}

/// HTML/CSS/JS for the terminal interface
const TERMINAL_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Manta AI Terminal</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }

        body {
            background: #0d1117;
            color: #c9d1d9;
            font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
            font-size: 14px;
            line-height: 1.6;
            height: 100vh;
            display: flex;
            flex-direction: column;
        }

        .header {
            background: #161b22;
            padding: 12px 20px;
            border-bottom: 1px solid #30363d;
            display: flex;
            align-items: center;
            justify-content: space-between;
        }

        .header h1 {
            font-size: 16px;
            color: #58a6ff;
            display: flex;
            align-items: center;
            gap: 8px;
        }

        .status {
            display: flex;
            align-items: center;
            gap: 8px;
            font-size: 12px;
        }

        .status-dot {
            width: 8px;
            height: 8px;
            border-radius: 50%;
            background: #238636;
            animation: pulse 2s infinite;
        }

        .status-dot.disconnected {
            background: #da3633;
            animation: none;
        }

        @keyframes pulse {
            0%, 100% { opacity: 1; }
            50% { opacity: 0.5; }
        }

        .terminal {
            flex: 1;
            overflow-y: auto;
            padding: 20px;
            display: flex;
            flex-direction: column;
            gap: 12px;
        }

        .message {
            max-width: 85%;
            padding: 12px 16px;
            border-radius: 8px;
            word-wrap: break-word;
        }

        .message.user {
            align-self: flex-end;
            background: #1f6feb;
            color: white;
        }

        .message.assistant {
            align-self: flex-start;
            background: #21262d;
            border: 1px solid #30363d;
        }

        .message.system {
            align-self: center;
            background: transparent;
            color: #8b949e;
            font-size: 12px;
            font-style: italic;
        }

        .message.error {
            align-self: center;
            background: #da3633;
            color: white;
            font-size: 12px;
        }

        .input-area {
            background: #161b22;
            border-top: 1px solid #30363d;
            padding: 16px 20px;
            display: flex;
            gap: 12px;
            align-items: center;
        }

        .input-wrapper {
            flex: 1;
            display: flex;
            align-items: center;
            background: #0d1117;
            border: 1px solid #30363d;
            border-radius: 8px;
            padding: 10px 16px;
            gap: 8px;
        }

        .input-wrapper:focus-within {
            border-color: #58a6ff;
        }

        .prompt {
            color: #58a6ff;
            font-weight: bold;
        }

        input {
            flex: 1;
            background: transparent;
            border: none;
            outline: none;
            color: #c9d1d9;
            font-family: inherit;
            font-size: 14px;
        }

        button {
            background: #238636;
            color: white;
            border: none;
            padding: 10px 20px;
            border-radius: 8px;
            cursor: pointer;
            font-family: inherit;
            font-size: 14px;
            transition: background 0.2s;
        }

        button:hover {
            background: #2ea043;
        }

        button:disabled {
            background: #30363d;
            cursor: not-allowed;
        }

        .typing-indicator {
            display: flex;
            gap: 4px;
            padding: 12px 16px;
            align-self: flex-start;
        }

        .typing-indicator span {
            width: 8px;
            height: 8px;
            background: #8b949e;
            border-radius: 50%;
            animation: bounce 1.4s infinite ease-in-out both;
        }

        .typing-indicator span:nth-child(1) { animation-delay: -0.32s; }
        .typing-indicator span:nth-child(2) { animation-delay: -0.16s; }

        @keyframes bounce {
            0%, 80%, 100% { transform: scale(0); }
            40% { transform: scale(1); }
        }

        pre {
            background: #0d1117;
            padding: 12px;
            border-radius: 6px;
            overflow-x: auto;
            margin: 8px 0;
        }

        code {
            font-family: 'Consolas', 'Monaco', 'Courier New', monospace;
            font-size: 13px;
        }

        .content pre code {
            color: #a5d6ff;
        }
    </style>
</head>
<body>
    <div class="header">
        <h1>🌊 Manta AI Terminal</h1>
        <div class="status">
            <div class="status-dot" id="status-dot"></div>
            <span id="status-text">Connecting...</span>
        </div>
    </div>

    <div class="terminal" id="terminal">
        <div class="message system">Connecting to Manta daemon...</div>
    </div>

    <div class="input-area">
        <div class="input-wrapper">
            <span class="prompt">&gt;</span>
            <input type="text" id="input" placeholder="Type your message..." disabled>
        </div>
        <button id="send" disabled>Send</button>
    </div>

    <script>
        const terminal = document.getElementById('terminal');
        const input = document.getElementById('input');
        const sendBtn = document.getElementById('send');
        const statusDot = document.getElementById('status-dot');
        const statusText = document.getElementById('status-text');

        // Connect to WebSocket
        const ws = new WebSocket(`ws://${window.location.host}/ws`);

        ws.onopen = () => {
            statusDot.classList.remove('disconnected');
            statusText.textContent = 'Connected';
            input.disabled = false;
            sendBtn.disabled = false;
            input.focus();
            addMessage('Connected to Manta AI Assistant. Type your message below.', 'system');
        };

        ws.onclose = () => {
            statusDot.classList.add('disconnected');
            statusText.textContent = 'Disconnected';
            input.disabled = true;
            sendBtn.disabled = true;
            addMessage('Connection lost. Please refresh the page.', 'error');
        };

        ws.onerror = (error) => {
            addMessage('WebSocket error occurred', 'error');
        };

        ws.onmessage = (event) => {
            hideTyping();
            try {
                const data = JSON.parse(event.data);
                if (data.error) {
                    addMessage('Error: ' + data.error, 'error');
                } else if (data.type === 'cron') {
                    addMessage(data.content, 'system');
                } else if (data.response) {
                    addMessage(data.response, 'assistant');
                }
            } catch (e) {
                addMessage(event.data, 'assistant');
            }
        };

        function addMessage(content, type) {
            const div = document.createElement('div');
            div.className = 'message ' + type;

            // Format code blocks
            content = content.replace(/```(\w+)?\n([\s\S]*?)```/g, '<pre><code>$2</code></pre>');
            content = content.replace(/`([^`]+)`/g, '<code>$1</code>');

            div.innerHTML = '<div class="content">' + content + '</div>';
            terminal.appendChild(div);
            terminal.scrollTop = terminal.scrollHeight;
        }

        function showTyping() {
            const existing = document.querySelector('.typing-indicator');
            if (existing) return;

            const div = document.createElement('div');
            div.className = 'typing-indicator';
            div.innerHTML = '<span></span><span></span><span></span>';
            terminal.appendChild(div);
            terminal.scrollTop = terminal.scrollHeight;
        }

        function hideTyping() {
            const typing = document.querySelector('.typing-indicator');
            if (typing) typing.remove();
        }

        function sendMessage() {
            const text = input.value.trim();
            if (!text || ws.readyState !== WebSocket.OPEN) return;

            addMessage(text, 'user');
            ws.send(JSON.stringify({ message: text }));
            input.value = '';
            showTyping();
        }

        sendBtn.onclick = sendMessage;
        input.onkeypress = (e) => {
            if (e.key === 'Enter') sendMessage();
        };
    </script>
</body>
</html>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_config_default() {
        let config = ServerConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 3000);
    }
}
