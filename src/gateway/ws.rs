//! WebSocket Protocol for Manta Gateway
//!
//! Provides an enhanced WebSocket protocol with:
//! - Token authentication on upgrade (via query parameter)
//! - Subscribe/unsubscribe by session_id for filtered event delivery
//! - Backward compatibility: clients that don't subscribe receive all events
//!
//! Mirrors OpenClaw's `src/gateway/websocket.ts` protocol.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::gateway::GatewayEvent;
use crate::gateway::GatewayState;

/// Query parameters for WebSocket upgrade
#[derive(Debug, Deserialize)]
pub struct WsConnectQuery {
    /// Authentication token (Bearer token or session token)
    pub token: Option<String>,
    /// Optional: pre-subscribe to a session on connect
    pub session_id: Option<String>,
}

/// Client -> Server WebSocket messages
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum WsClientMessage {
    /// Subscribe to events for specific session IDs
    Subscribe { session_ids: Vec<String> },
    /// Unsubscribe from specific session IDs
    Unsubscribe { session_ids: Vec<String> },
    /// Subscribe to all events (reset filter)
    SubscribeAll,
    /// Ping to keep connection alive
    Ping,
    /// Acknowledge receipt of events
    Ack { last_event_id: Option<String> },
}

/// Server -> Client WebSocket messages
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum WsServerMessage {
    /// Connected successfully
    Connected { session_count: usize },
    /// Event delivery
    Event { data: GatewayEvent },
    /// Subscription confirmed
    Subscribed { session_ids: Vec<String> },
    /// Unsubscription confirmed
    Unsubscribed { session_ids: Vec<String> },
    /// Error message
    Error { message: String },
    /// Pong response
    Pong,
}

/// WebSocket connection state
#[allow(dead_code)]
struct WsConnection {
    /// Subscribed session IDs (empty = all events)
    subscriptions: HashSet<String>,
    /// Whether the client is authenticated
    authenticated: bool,
    /// User ID if authenticated
    user_id: Option<String>,
    /// Command channel for subscription updates
    cmd_tx: mpsc::Sender<WsCommand>,
}

/// Internal commands for the WebSocket task
#[derive(Debug, Clone)]
enum WsCommand {
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
    SubscribeAll,
}

/// Handler: WebSocket upgrade with token validation
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<WsConnectQuery>,
) -> impl IntoResponse {
    // Validate token if auth is required
    let auth_required = {
        let config = state.config.read().await;
        config.security.auth_required
    };

    let mut user_id = None;
    let mut authenticated = false;

    if auth_required {
        // Try Bearer token from query
        if let Some(ref token) = query.token {
            if let Some(session) = state.auth_manager.validate_session(token).await {
                user_id = Some(session.user_id.0.clone());
                authenticated = true;
                debug!("WebSocket authenticated via query token for user: {}", session.user_id);
            }
        }

        // If not authenticated via query, reject (can't read cookies/here easily in upgrade)
        if !authenticated {
            warn!("WebSocket upgrade rejected: missing or invalid token");
            return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
        }
    }

    ws.on_upgrade(move |socket| {
        handle_websocket(socket, state, authenticated, user_id, query.session_id)
    })
}

/// Main WebSocket handler
async fn handle_websocket(
    mut socket: WebSocket,
    state: Arc<GatewayState>,
    authenticated: bool,
    user_id: Option<String>,
    initial_session: Option<String>,
) {
    info!(
        "Gateway events WebSocket connected (auth={}, user={:?})",
        authenticated, user_id
    );

    // Subscribe to gateway events
    let mut event_rx = state.event_tx.subscribe();

    // Command channel for subscription updates
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<WsCommand>(100);

    // Connection state
    let mut subscriptions: HashSet<String> = HashSet::new();
    if let Some(session_id) = initial_session {
        subscriptions.insert(session_id);
    }

    // Send connected confirmation
    let connected_msg = WsServerMessage::Connected {
        session_count: subscriptions.len(),
    };
    let _ = socket
        .send(Message::Text(serde_json::to_string(&connected_msg).unwrap_or_default()))
        .await;

    // Wrap socket in Arc<Mutex> for shared access between tasks
    let socket = Arc::new(tokio::sync::Mutex::new(socket));
    let socket_recv = socket.clone();

    // Task to receive gateway events and send to client (filtered by subscription)
    let send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Ok(event) = event_rx.recv() => {
                    // Filter by subscription if any are set
                    let should_send = if subscriptions.is_empty() {
                        true // No filter = all events
                    } else {
                        // Check if event matches any subscribed session
                        match &event {
                            GatewayEvent::AgentResponse { session_id, .. }
                            | GatewayEvent::ToolCalling { session_id, .. }
                            | GatewayEvent::ToolResult { session_id, .. }
                            | GatewayEvent::Completed { session_id, .. }
                            | GatewayEvent::ProcessingError { session_id, .. }
                            | GatewayEvent::Thinking { session_id, .. } => {
                                subscriptions.contains(session_id)
                            }
                            _ => true, // Global events always sent
                        }
                    };

                    if should_send {
                        let msg = WsServerMessage::Event { data: event };
                        let text = serde_json::to_string(&msg).unwrap_or_default();
                        let mut sock = socket.lock().await;
                        if sock.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                }
                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        WsCommand::Subscribe(ids) => {
                            for id in ids {
                                subscriptions.insert(id);
                            }
                        }
                        WsCommand::Unsubscribe(ids) => {
                            for id in ids {
                                subscriptions.remove(&id);
                            }
                        }
                        WsCommand::SubscribeAll => {
                            subscriptions.clear();
                        }
                    }
                }
                else => break,
            }
        }
    });

    // Task to receive client messages (commands, ping/pong)
    let recv_task = tokio::spawn(async move {
        loop {
            let msg = {
                let mut sock = socket_recv.lock().await;
                sock.recv().await
            };
            match msg {
                Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Ping(data))) => {
                    debug!("Received ping: {:?}", data);
                }
                Some(Ok(Message::Text(text))) => {
                    debug!("Received WebSocket message: {}", text);

                    match serde_json::from_str::<WsClientMessage>(&text) {
                        Ok(cmd) => {
                            let ws_cmd = match cmd {
                                WsClientMessage::Subscribe { session_ids } => {
                                    Some(WsCommand::Subscribe(session_ids))
                                }
                                WsClientMessage::Unsubscribe { session_ids } => {
                                    Some(WsCommand::Unsubscribe(session_ids))
                                }
                                WsClientMessage::SubscribeAll => Some(WsCommand::SubscribeAll),
                                WsClientMessage::Ping => {
                                    let pong = serde_json::to_string(&WsServerMessage::Pong)
                                        .unwrap_or_default();
                                    let mut sock = socket_recv.lock().await;
                                    let _ = sock.send(Message::Text(pong)).await;
                                    continue;
                                }
                                WsClientMessage::Ack { .. } => {
                                    // Acknowledgments are no-ops for now
                                    continue;
                                }
                            };

                            if let Some(ws_cmd) = ws_cmd {
                                if cmd_tx.send(ws_cmd).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse WebSocket message: {}", e);
                        }
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
                None => break,
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = send_task => {}
        _ = recv_task => {}
    }

    info!("Gateway events WebSocket disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_client_message_deserialize() {
        let json = r#"{"type":"subscribe","session_ids":["s1","s2"]}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::Subscribe { session_ids } => {
                assert_eq!(session_ids, vec!["s1", "s2"]);
            }
            _ => panic!("Expected Subscribe"),
        }
    }

    #[test]
    fn test_ws_server_message_serialize() {
        let msg = WsServerMessage::Pong;
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("pong"));
    }

    #[test]
    fn test_ws_client_message_unsubscribe() {
        let json = r#"{"type":"unsubscribe","session_ids":["s1"]}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMessage::Unsubscribe { session_ids } => {
                assert_eq!(session_ids, vec!["s1"]);
            }
            _ => panic!("Expected Unsubscribe"),
        }
    }

    #[test]
    fn test_ws_client_message_ping() {
        let json = r#"{"type":"ping"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMessage::Ping));
    }

    #[test]
    fn test_ws_client_message_subscribe_all() {
        let json = r#"{"type":"subscribe_all"}"#;
        let msg: WsClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, WsClientMessage::SubscribeAll));
    }

    #[test]
    fn test_ws_server_message_connected() {
        let msg = WsServerMessage::Connected { session_count: 3 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("connected"));
        assert!(json.contains("3"));
    }

    #[test]
    fn test_ws_server_message_error() {
        let msg = WsServerMessage::Error { message: "oops".to_string() };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("oops"));
    }

    #[test]
    fn test_ws_connect_query_deserialize() {
        let query: WsConnectQuery = serde_json::from_str(r#"{"token":"abc","session_id":"s1"}"#).unwrap();
        assert_eq!(query.token, Some("abc".to_string()));
        assert_eq!(query.session_id, Some("s1".to_string()));
    }
}
