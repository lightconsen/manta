//! Web Terminal for Manta
//!
//! Provides a browser-based terminal interface for interacting with the AI assistant.
//! Uses WebSockets for real-time bidirectional communication.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Html,
    routing::get,
    Router,
};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info};

use crate::agent::Agent;
use crate::channels::IncomingMessage;
use crate::client::DaemonClient;
use crate::server::{broadcast_cron_output, init_cron_broadcast};
use std::sync::Arc as StdArc;

// Re-export broadcast functions from server module

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
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket_daemon(socket, state))
}

/// Handle WebSocket connection with daemon
async fn handle_socket_daemon(mut socket: WebSocket, state: DaemonWebState) {
    info!("New WebSocket connection (daemon mode)");

    // Subscribe to cron broadcasts
    let mut cron_rx = init_cron_broadcast().await;

    // Generate conversation ID for this session
    let conversation_id = uuid::Uuid::new_v4().to_string();

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "system",
        "content": "Connected to Manta AI Assistant (via daemon). Type your message below."
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

                        // Send typing indicator
                        let typing = serde_json::json!({
                            "type": "typing",
                            "content": true
                        });
                        if socket.send(Message::Text(typing.to_string())).await.is_err() {
                            break;
                        }

                        // Process message via daemon
                        match state.client.chat(&text, Some(&conversation_id)).await {
                            Ok(response) => {
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
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle WebSocket connection
async fn handle_socket(mut socket: WebSocket, state: WebTerminalState) {
    info!("New WebSocket connection established");

    // Subscribe to cron broadcasts
    let mut cron_rx = init_cron_broadcast().await;

    // Generate conversation ID for this session
    let conversation_id = uuid::Uuid::new_v4().to_string();

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "system",
        "content": "Connected to Manta AI Assistant. Type your message below."
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
        assert!(html.contains("settingsBtn"), "HTML should contain settings button");
        assert!(html.contains("⚙️"), "HTML should contain settings icon");
    }

    #[test]
    fn test_terminal_html_contains_version_span() {
        let html = terminal_html();
        assert!(html.contains("versionText"), "HTML should contain version span");
        assert!(html.contains("header-center"), "HTML should contain header-center div");
    }
}