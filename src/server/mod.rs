//! HTTP Server for Syscity
//!
//! Provides REST API endpoints and WebSocket for interacting with the Syscity AI assistant.

use crate::core::Engine;
use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3000,
        }
    }
}

/// Start the HTTP server with agent
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

    // Start API server (includes frontend routes)
    let api_app = create_api_router(state);
    let api_addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| crate::error::SyscityError::Validation(format!("Invalid address: {}", e)))?;

    info!("Starting API server on {}", api_addr);
    println!("🌐 Server available at http://localhost:{}", config.port);

    let api_listener = TcpListener::bind(&api_addr)
        .await
        .map_err(|e| crate::error::SyscityError::Internal(format!("Failed to bind API: {}", e)))?;

    axum::serve(api_listener, api_app)
        .await
        .map_err(|e| crate::error::SyscityError::Internal(format!("Server error: {}", e)))?;

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
    "Syscity Webhook Server\n\nAvailable endpoints:\n- /webhooks/whatsapp - WhatsApp Business API webhooks\n- /webhooks/lark - Lark/Feishu webhooks\n- /webhooks/qq - QQ Bot webhooks\n"
}

/// Root endpoint
async fn root(State(state): State<AppState>) -> impl IntoResponse {
    let agent_status = if state.agent.is_some() {
        "available"
    } else {
        "not configured"
    };

    Json(serde_json::json!({
        "name": "Syscity",
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
                                    crate::agent::ProgressEvent::ToolResult { name, result, data: _ } => {
                                        serde_json::json!({
                                            "type": "progress",
                                            "status": "tool_result",
                                            "tool": name,
                                            "result": result
                                        })
                                    }
                                    crate::agent::ProgressEvent::Generating { .. } => {
                                        serde_json::json!({"type": "progress", "status": "generating"})
                                    }
                                    crate::agent::ProgressEvent::ContentDelta { .. } => {
                                        serde_json::json!({"type": "progress", "status": "streaming"})
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
