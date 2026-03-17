//! Web Terminal for Manta
//!
//! Provides a browser-based terminal interface for interacting with the AI assistant.
//! Uses WebSockets for real-time bidirectional communication.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::Html,
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{debug, error, info};

use crate::agent::Agent;
use crate::channels::IncomingMessage;
use crate::client::DaemonClient;
use crate::server::init_cron_broadcast;

// Re-export broadcast functions from server module

/// Query parameters for WebSocket connection
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    /// Start a new conversation (true/false)
    pub new: Option<bool>,
    /// Specific conversation ID to resume
    pub conversation: Option<String>,
}

/// Shared application state
#[derive(Clone)]
pub struct WebTerminalState {
    pub agent: Arc<Agent>,
}

/// Start the web terminal server
pub async fn start_web_terminal(agent: Arc<Agent>, port: u16) -> crate::Result<()> {
    // Initialize broadcast channel
    let _ = init_cron_broadcast().await;

    let state = WebTerminalState { agent };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("🌐 Web Terminal starting on http://{}", addr);
    println!("🌐 Open your browser and navigate to http://localhost:{}", port);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// State for daemon-connected web terminal
#[derive(Clone)]
pub struct DaemonWebState {
    pub client: DaemonClient,
}

/// Start web terminal that connects to daemon
pub async fn start_web_terminal_with_daemon(client: DaemonClient, port: u16) -> crate::Result<()> {
    // Initialize broadcast channel
    let _ = init_cron_broadcast().await;

    let state = DaemonWebState { client };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler_daemon))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("🌐 Web Terminal (daemon mode) starting on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// WebSocket upgrade handler for daemon mode
async fn ws_handler_daemon(
    ws: WebSocketUpgrade,
    State(state): State<DaemonWebState>,
    Query(query): Query<WsQuery>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket_daemon(socket, state, query))
}

/// Handle WebSocket connection with daemon
async fn handle_socket_daemon(mut socket: WebSocket, state: DaemonWebState, query: WsQuery) {
    info!("New WebSocket connection (daemon mode), query: {:?}", query);

    // Subscribe to cron broadcasts
    let mut cron_rx = init_cron_broadcast().await;

    // Determine initial conversation ID based on query parameters
    let mut conversation_id: Option<String> = if query.new == Some(true) {
        // Force new conversation
        Some(uuid::Uuid::new_v4().to_string())
    } else if let Some(conv) = query.conversation {
        // Use specified conversation
        Some(conv)
    } else {
        // Try to get last conversation from daemon
        match state.client.get_last_conversation("web_user").await {
            Ok(resp) => {
                if let Some(conv_id) = resp.conversation_id {
                    info!("Resuming last conversation: {}", conv_id);
                    Some(conv_id)
                } else {
                    info!("No previous conversation found, starting new");
                    None
                }
            }
            Err(e) => {
                debug!("Could not get last conversation: {}", e);
                None
            }
        }
    };

    // Load and send chat history if available
    if let Some(ref conv_id) = conversation_id {
        match state.client.get_chat_history(conv_id, 100).await {
            Ok(history) => {
                if !history.messages.is_empty() {
                    let history_json = serde_json::json!({
                        "type": "history",
                        "conversation_id": conv_id,
                        "messages": history.messages
                    });
                    if socket.send(Message::Text(history_json.to_string())).await.is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                debug!("Could not load chat history: {}", e);
            }
        }
    }

    // Send welcome message
    let welcome_msg = if conversation_id.is_some() {
        format!("Connected to Manta AI Assistant (via daemon).\nType /new to start a fresh conversation.")
    } else {
        "Connected to Manta AI Assistant (via daemon).\nType /new to start a fresh conversation.".to_string()
    };
    let welcome = serde_json::json!({
        "type": "system",
        "content": welcome_msg
    });
    if let Err(e) = socket.send(Message::Text(welcome.to_string())).await {
        error!("Failed to send welcome: {}", e);
        return;
    }

    // Main message processing loop
    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        info!("Received message: {}", text);

                        // Handle /new command to start a new session
                        if text.trim() == "/new" {
                            let new_id = uuid::Uuid::new_v4().to_string();
                            conversation_id = Some(new_id.clone());
                            let system_msg = serde_json::json!({
                                "type": "system",
                                "content": format!("🆕 Started new conversation: {}", new_id)
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

                        // Process message via daemon
                        // Pass None on first message to let daemon pick last conversation
                        match state.client.chat(&text, conversation_id.as_deref()).await {
                            Ok(response) => {
                                // Store the conversation ID from the response
                                conversation_id = Some(response.conversation_id.clone());

                                let resp_json = serde_json::json!({
                                    "type": "message",
                                    "role": "assistant",
                                    "content": response.response
                                });
                                if socket.send(Message::Text(resp_json.to_string())).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("Daemon error: {}", e);
                                let error_json = serde_json::json!({
                                    "type": "error",
                                    "content": format!("Error: {}", e)
                                });
                                if socket.send(Message::Text(error_json.to_string())).await.is_err() {
                                    break;
                                }
                            }
                        }

                        // Send typing indicator off
                        let typing_off = serde_json::json!({
                            "type": "typing",
                            "content": false
                        });
                        if socket.send(Message::Text(typing_off.to_string())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        info!("WebSocket connection closed");
                        break;
                    }
                    _ => {}
                }
            }

            // Handle cron broadcasts (plain text with 📅 prefix)
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

/// HTML page with terminal interface
async fn index_handler() -> Html<String> {
    Html(terminal_html())
}

/// WebSocket upgrade handler
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<WebTerminalState>,
    Query(query): Query<WsQuery>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, query))
}

/// Handle WebSocket connection
async fn handle_socket(mut socket: WebSocket, state: WebTerminalState, query: WsQuery) {
    info!("New WebSocket connection established, query: {:?}", query);

    // Subscribe to cron broadcasts
    let mut cron_rx = init_cron_broadcast().await;

    // Determine conversation ID based on query parameters
    let mut conversation_id = if query.new == Some(true) {
        // Force new conversation
        let new_id = uuid::Uuid::new_v4().to_string();
        info!("Starting new conversation (new=true): {}", new_id);
        new_id
    } else if let Some(conv) = query.conversation {
        // Use specified conversation
        info!("Using specified conversation: {}", conv);
        conv
    } else {
        // Get last conversation or generate new one
        match state.agent.get_last_conversation("user").await {
            Ok(Some(last_conv)) => {
                info!("Resuming last conversation: {}", last_conv);
                last_conv
            }
            _ => {
                let new_id = uuid::Uuid::new_v4().to_string();
                info!("Starting new conversation: {}", new_id);
                new_id
            }
        }
    };

    // Load and send chat history if available
    match state.agent.get_chat_history(&conversation_id, 100).await {
        Ok(history) => {
            if !history.is_empty() {
                let history_json = serde_json::json!({
                    "type": "history",
                    "conversation_id": &conversation_id,
                    "messages": history
                });
                if socket.send(Message::Text(history_json.to_string())).await.is_err() {
                    return;
                }
            }
        }
        Err(e) => {
            debug!("Could not load chat history: {}", e);
        }
    }

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "system",
        "content": "Connected to Manta AI Assistant.\nType /new to start a fresh conversation."
    });
    if let Err(e) = socket.send(Message::Text(welcome.to_string())).await {
        error!("Failed to send welcome: {}", e);
        return;
    }

    // Main message processing loop with cron broadcast handling
    loop {
        tokio::select! {
            // Handle incoming WebSocket messages
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        info!("Received message: {}", text);

                        // Handle /new command to start a new session
                        if text.trim() == "/new" {
                            conversation_id = uuid::Uuid::new_v4().to_string();
                            let system_msg = serde_json::json!({
                                "type": "system",
                                "content": format!("🆕 Started new conversation: {}", conversation_id)
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

                        // Process message with agent
                        let incoming = IncomingMessage::new(
                            "user",
                            &conversation_id,
                            &text
                        );

                        match state.agent.process_message(incoming).await {
                            Ok(response) => {
                                let resp_json = serde_json::json!({
                                    "type": "message",
                                    "role": "assistant",
                                    "content": response.content
                                });
                                if socket.send(Message::Text(resp_json.to_string())).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("Agent error: {}", e);
                                let error_json = serde_json::json!({
                                    "type": "error",
                                    "content": format!("Error: {}", e)
                                });
                                if socket.send(Message::Text(error_json.to_string())).await.is_err() {
                                    break;
                                }
                            }
                        }

                        // Send typing indicator off
                        let typing_off = serde_json::json!({
                            "type": "typing",
                            "content": false
                        });
                        if socket.send(Message::Text(typing_off.to_string())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                        info!("WebSocket connection closed");
                        break;
                    }
                    _ => {}
                }
            }

            // Handle cron broadcasts (plain text with 📅 prefix)
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

/// HTML/CSS/JS for the terminal interface (loaded from assets/web_terminal.html)
fn terminal_html() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let html = include_str!("../assets/web_terminal.html");
    html.replace("{VERSION}", version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_html_contains_version() {
        let html = terminal_html();
        let version = env!("CARGO_PKG_VERSION");
        assert!(html.contains(&format!("v{}", version)),
            "HTML should contain version v{}", version);
    }

    #[test]
    fn test_terminal_html_contains_settings_button() {
        let html = terminal_html();
        assert!(html.contains("settings-btn"), "HTML should contain settings button class");
        assert!(html.contains("⚙️"), "HTML should contain settings icon");
    }

    #[test]
    fn test_terminal_html_contains_version_span() {
        let html = terminal_html();
        assert!(html.contains("version"), "HTML should contain version class");
        assert!(html.contains("header-center"), "HTML should contain header-center div");
    }
}