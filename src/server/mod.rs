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
        .route("/webhooks", get(webhook_root))
        .with_state(state)
}

/// Webhook root endpoint
async fn webhook_root() -> &'static str {
    "Manta Webhook Server\n\nAvailable endpoints:\n- /webhooks/whatsapp - WhatsApp Business API webhooks\n- /webhooks/lark - Lark/Feishu webhooks\n- /webhooks/qq - QQ Bot webhooks\n"
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
    let agent_status = if state.agent.is_some() {
        "ready"
    } else {
        "disabled"
    };

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

/// History request
#[derive(Debug, Deserialize)]
pub struct HistoryRequest {
    pub conversation_id: String,
    pub limit: Option<usize>,
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
            None => match agent.get_last_conversation("user").await {
                Ok(Some(last_conv)) => last_conv,
                _ => crate::channels::ConversationId::generate().to_string(),
            },
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
async fn chat_stream(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    if state.agent.is_none() {
        return (StatusCode::SERVICE_UNAVAILABLE, "AI agent not configured").into_response();
    }

    ws.on_upgrade(|socket| handle_chat_socket(socket, state))
}

/// Handle WebSocket chat (for CLI and Web)
async fn handle_chat_socket(mut socket: axum::extract::ws::WebSocket, state: AppState) {
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
                        // Try to parse as a generic JSON first to check message type
                        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&text) {
                            // Check if it's a history request
                            if json_val.get("type").and_then(|v| v.as_str()) == Some("load_history") {
                                if let Some(cid) = json_val.get("conversation_id").and_then(|v| v.as_str()) {
                                    conversation_id = Some(cid.to_string());

                                    // Load and send history
                                    if let Some(agent) = &state.agent {
                                        let limit = json_val.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                                        match agent.get_chat_history(cid, limit).await {
                                            Ok(history) => {
                                                let history_msg = serde_json::json!({
                                                    "type": "history",
                                                    "conversation_id": cid,
                                                    "messages": history.iter().map(|msg| {
                                                        let created_at_secs = msg.created_at
                                                            .duration_since(std::time::UNIX_EPOCH)
                                                            .unwrap_or_default()
                                                            .as_secs();
                                                        serde_json::json!({
                                                            "id": msg.id,
                                                            "role": msg.role,
                                                            "content": msg.content,
                                                            "created_at": created_at_secs
                                                        })
                                                    }).collect::<Vec<_>>()
                                                });
                                                if socket.send(Message::Text(history_msg.to_string())).await.is_err() {
                                                    break;
                                                }
                                            }
                                            Err(e) => {
                                                error!("Failed to load history: {}", e);
                                            }
                                        }
                                    }
                                }
                                continue;
                            }
                        }

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

                            // Process message with progress updates
                            use tokio::sync::mpsc;
                            let (progress_tx, mut progress_rx) = mpsc::channel::<crate::agent::ProgressEvent>(32);

                            // Create progress callback
                            let progress_cb: crate::agent::ProgressCallback = Arc::new(
                                move |event: crate::agent::ProgressEvent| {
                                    let tx = progress_tx.clone();
                                    Box::pin(async move {
                                        let _ = tx.send(event).await;
                                    })
                                }
                            );

                            // Process in a spawned task so we can concurrently receive progress
                            let agent_clone = agent.clone();
                            let process_handle = tokio::spawn(async move {
                                agent_clone.process_message_with_progress(incoming, progress_cb).await
                            });

                            // Forward progress events to WebSocket
                            while let Some(event) = progress_rx.recv().await {
                                let msg = match &event {
                                    crate::agent::ProgressEvent::Started => {
                                        serde_json::json!({"type": "progress", "status": "started"})
                                    }
                                    crate::agent::ProgressEvent::ToolCalling { name, arguments } => {
                                        serde_json::json!({
                                            "type": "progress",
                                            "status": "tool_calling",
                                            "tool": name,
                                            "arguments": arguments
                                        })
                                    }
                                    crate::agent::ProgressEvent::ToolResult { name, result } => {
                                        serde_json::json!({
                                            "type": "progress",
                                            "status": "tool_result",
                                            "tool": name,
                                            "result": result
                                        })
                                    }
                                    crate::agent::ProgressEvent::Generating => {
                                        serde_json::json!({"type": "progress", "status": "generating"})
                                    }
                                    crate::agent::ProgressEvent::Completed { .. } => {
                                        serde_json::json!({"type": "progress", "status": "completed"})
                                    }
                                    crate::agent::ProgressEvent::Error { message } => {
                                        serde_json::json!({"type": "progress", "status": "error", "error": message})
                                    }
                                };
                                if socket.send(Message::Text(msg.to_string())).await.is_err() {
                                    break;
                                }
                                // Stop on completed/error
                                if matches!(event, crate::agent::ProgressEvent::Completed { .. } | crate::agent::ProgressEvent::Error { .. }) {
                                    break;
                                }
                            }

                            // Get final result
                            match process_handle.await {
                                Ok(Ok(response)) => {
                                    let resp = ChatResponse {
                                        response: response.content,
                                        conversation_id: cid,
                                    };
                                    let _ = socket.send(Message::Text(
                                        serde_json::to_string(&resp).unwrap_or_default()
                                    )).await;
                                }
                                Ok(Err(e)) => {
                                    let _ = socket.send(Message::Text(
                                        format!("{{\"error\": \"{}\"}}", e)
                                    )).await;
                                }
                                Err(e) => {
                                    let _ = socket.send(Message::Text(
                                        format!("{{\"error\": \"Task failed: {}\"}}", e)
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
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
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
            Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e.to_string()}))),
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
                Err(e) => {
                    (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": e.to_string()})))
                }
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

        .message.progress {
            align-self: flex-start;
            background: #161b22;
            border: 1px solid #30363d;
            color: #c9d1d9;
            font-size: 13px;
            max-width: 95%;
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

        /* Markdown Table Styling */
        .content table {
            width: 100%;
            border-collapse: collapse;
            margin: 16px 0;
            font-size: 13px;
            border-radius: 6px;
            overflow: hidden;
        }

        .content table th {
            background: #21262d;
            color: #58a6ff;
            font-weight: 600;
            text-align: left;
            padding: 12px 16px;
            border-bottom: 2px solid #30363d;
        }

        .content table td {
            padding: 10px 16px;
            border-bottom: 1px solid #21262d;
        }

        .content table tr:last-child td {
            border-bottom: none;
        }

        .content table tr:nth-child(even) {
            background: #161b22;
        }

        .content table tr:hover {
            background: #21262d;
        }

        /* Header Styling */
        .content h1 {
            color: #58a6ff;
            font-size: 20px;
            font-weight: 600;
            margin: 20px 0 12px 0;
            padding-bottom: 8px;
            border-bottom: 2px solid #30363d;
        }

        .content h2 {
            color: #79c0ff;
            font-size: 17px;
            font-weight: 600;
            margin: 18px 0 10px 0;
            padding-bottom: 6px;
            border-bottom: 1px solid #30363d;
        }

        .content h3 {
            color: #a5d6ff;
            font-size: 15px;
            font-weight: 600;
            margin: 14px 0 8px 0;
        }

        .content h4 {
            color: #c9d1d9;
            font-size: 14px;
            font-weight: 600;
            margin: 12px 0 6px 0;
        }

        /* List Styling */
        .content ul, .content ol {
            margin: 10px 0;
            padding-left: 24px;
        }

        .content li {
            margin: 6px 0;
            line-height: 1.6;
        }

        .content li::marker {
            color: #58a6ff;
        }

        /* Blockquote Styling */
        .content blockquote {
            border-left: 4px solid #58a6ff;
            margin: 12px 0;
            padding: 8px 16px;
            background: #161b22;
            border-radius: 0 6px 6px 0;
            color: #b0b8c4;
        }

        /* Horizontal Rule */
        .content hr {
            border: none;
            height: 1px;
            background: linear-gradient(to right, transparent, #30363d, transparent);
            margin: 20px 0;
        }

        /* Bold and Italic */
        .content strong {
            color: #f0f6fc;
            font-weight: 600;
        }

        .content em {
            color: #b0b8c4;
            font-style: italic;
        }

        /* Links */
        .content a {
            color: #58a6ff;
            text-decoration: none;
        }

        .content a:hover {
            text-decoration: underline;
        }

        /* Time Display in Header */
        .header-time {
            font-size: 12px;
            color: #8b949e;
            font-family: inherit;
        }
    </style>
    <!-- Load marked.js for Markdown parsing -->
    <script src="https://cdn.jsdelivr.net/npm/marked/marked.min.js"></script>
</head>
<body>
    <div class="header">
        <h1>🌊 Manta AI Terminal</h1>
        <div class="header-time" id="header-time"></div>
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
        const headerTime = document.getElementById('header-time');

        // Update time display
        function updateTime() {
            const now = new Date();
            const timeStr = now.toLocaleString('zh-CN', {
                year: 'numeric',
                month: '2-digit',
                day: '2-digit',
                hour: '2-digit',
                minute: '2-digit',
                second: '2-digit',
                hour12: false
            });
            if (headerTime) {
                headerTime.textContent = '🕐 ' + timeStr;
            }
        }
        updateTime();
        setInterval(updateTime, 1000);

        // Restore conversation ID from localStorage
        let conversationId = localStorage.getItem('manta_conversation_id');

        // Connect to WebSocket
        const ws = new WebSocket(`ws://${window.location.host}/ws`);

        ws.onopen = () => {
            statusDot.classList.remove('disconnected');
            statusText.textContent = 'Connected';
            input.disabled = false;
            sendBtn.disabled = false;
            input.focus();

            // If we have a stored conversation, load its history
            if (conversationId) {
                ws.send(JSON.stringify({
                    type: 'load_history',
                    conversation_id: conversationId,
                    limit: 50
                }));
            } else {
                addMessage('Connected to Manta AI Assistant. Type your message below.', 'system');
            }
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
            try {
                const data = JSON.parse(event.data);
                if (data.error) {
                    hideTyping();
                    addMessage('Error: ' + data.error, 'error');
                } else if (data.type === 'cron') {
                    addMessage(data.content, 'system');
                } else if (data.type === 'progress') {
                    handleProgress(data);
                } else if (data.type === 'history') {
                    // Display history messages
                    displayHistory(data.messages);
                    // Store conversation ID
                    conversationId = data.conversation_id;
                    localStorage.setItem('manta_conversation_id', conversationId);
                    addMessage('Loaded ' + data.messages.length + ' messages from previous conversation.', 'system');
                } else if (data.response) {
                    hideTyping();
                    addMessage(data.response, 'assistant');
                    // Store conversation ID from response
                    if (data.conversation_id) {
                        conversationId = data.conversation_id;
                        localStorage.setItem('manta_conversation_id', conversationId);
                    }
                }
            } catch (e) {
                hideTyping();
                addMessage(event.data, 'assistant');
            }
        };

        // Display history messages
        function displayHistory(messages) {
            // Clear existing messages first (except system messages)
            const existingMessages = terminal.querySelectorAll('.message:not(.system)');
            existingMessages.forEach(m => m.remove());

            messages.forEach(msg => {
                const role = msg.role;
                const content = msg.content;
                if (role === 'user') {
                    addMessage(content, 'user');
                } else if (role === 'assistant') {
                    addMessage(content, 'assistant');
                }
            });
        }

        // Handle progress updates
        let currentProgressDiv = null;

        function handleProgress(data) {
            switch(data.status) {
                case 'started':
                    showTyping();
                    break;
                case 'tool_calling':
                    if (!currentProgressDiv) {
                        currentProgressDiv = document.createElement('div');
                        currentProgressDiv.className = 'message progress';
                        terminal.appendChild(currentProgressDiv);
                    }
                    // Try to parse and format arguments
                    let argsStr = data.arguments || '{}';
                    try {
                        const argsObj = JSON.parse(argsStr);
                        argsStr = JSON.stringify(argsObj, null, 2);
                    } catch (e) {
                        // Keep original if parse fails
                    }
                    const displayArgs = argsStr.substring(0, 200);
                    currentProgressDiv.innerHTML = '<div class="content">🔧 Using tool: <strong>' + data.tool + '</strong><br/><pre style="margin: 4px 0; padding: 8px; background: #161b22; border-radius: 4px; overflow-x: auto;"><code>' + displayArgs + '</code></pre></div>';
                    terminal.scrollTop = terminal.scrollHeight;
                    break;
                case 'tool_result':
                    if (currentProgressDiv) {
                        const result = data.result ? data.result.substring(0, 200) : 'done';
                        currentProgressDiv.innerHTML += '<div class="content" style="margin-top: 4px; color: #58a6ff;">✓ Result: ' + result + '...</div>';
                        terminal.scrollTop = terminal.scrollHeight;
                    }
                    break;
                case 'generating':
                    if (currentProgressDiv) {
                        currentProgressDiv.innerHTML += '<div class="content" style="margin-top: 4px;">💭 Generating response...</div>';
                        terminal.scrollTop = terminal.scrollHeight;
                        currentProgressDiv = null;  // Don't update this one anymore
                    }
                    showTyping();
                    break;
                case 'completed':
                    hideTyping();
                    currentProgressDiv = null;
                    break;
                case 'error':
                    hideTyping();
                    addMessage('Error: ' + data.error, 'error');
                    currentProgressDiv = null;
                    break;
            }
        }

        function addMessage(content, type) {
            const div = document.createElement('div');
            div.className = 'message ' + type;

            let formattedContent;
            if (type === 'user') {
                // User messages are plain text
                formattedContent = escapeHtml(content);
            } else if (typeof marked !== 'undefined') {
                // Use marked.js for markdown parsing (assistant and system messages)
                formattedContent = marked.parse(content, {
                    breaks: true,
                    gfm: true,
                    headerIds: false
                });
            } else {
                // Fallback if marked.js isn't loaded
                formattedContent = escapeHtml(content)
                    .replace(/\n/g, '<br>');
            }

            div.innerHTML = '<div class="content">' + formattedContent + '</div>';
            terminal.appendChild(div);
            terminal.scrollTop = terminal.scrollHeight;
        }

        // Escape HTML to prevent XSS
        function escapeHtml(text) {
            const div = document.createElement('div');
            div.textContent = text;
            return div.innerHTML;
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
