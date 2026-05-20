//! WebSocket Protocol for Manta Gateway
//!
//! Implements the WebSocket-native RPC protocol (docs/protocol.md).
//!
//! Protocol flow:
//!   1. Client opens WebSocket to /ws
//!   2. Server accepts (no auth yet)
//!   3. Client sends `connect` req as first frame
//!   4. Server validates auth + protocol version, replies `hello-ok`
//!   5. Client sends method calls (e.g. `chat.send`), server replies `res`
//!   6. Server pushes events (`chat.delta`, `tool.calling`, etc.) asynchronously

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
};
use futures_util::{stream::{SplitSink, SplitStream}, SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::gateway::protocol::*;
use crate::gateway::{GatewayEvent, GatewayState};
use crate::security::UserId;

// ── Query Parameters ──────────────────────────────────────────────────────────

/// Query parameters for WebSocket upgrade
#[derive(Debug, Deserialize)]
pub struct WsConnectQuery {
    /// Optional: pre-subscribe to a session on connect
    pub session_id: Option<String>,
    /// Optional: client identifier hint
    pub client: Option<String>,
}

// ── Internal Commands ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum WsCommand {
    /// Send a response frame to the client
    SendResponse(String),
    /// Send an event frame to the client
    SendEvent(String),
    /// Subscription updates
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
    SubscribeAll,
}

// ── Public Handler ────────────────────────────────────────────────────────────

/// Handler: WebSocket upgrade (no auth at this stage)
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<WsConnectQuery>,
) -> impl IntoResponse {
    // Extract auth mode
    let auth_mode = {
        let config = state.config.read().await;
        config.security.auth_mode.clone()
    };

    ws.on_upgrade(move |socket| {
        handle_websocket(socket, state, query, auth_mode)
    })
}

// ── Main WebSocket Loop ───────────────────────────────────────────────────────

async fn handle_websocket(
    socket: WebSocket,
    state: Arc<GatewayState>,
    query: WsConnectQuery,
    auth_mode: crate::gateway::protocol::AuthMode,
) {
    let conn_id = Uuid::new_v4().to_string();
    info!("[{}] WebSocket connected", conn_id);

    // Connection state — shared between send and recv tasks
    let mut proto_conn = ProtocolConnection::new(conn_id.clone());
    if let Some(sid) = query.session_id {
        proto_conn.subscriptions.push(sid);
    }
    let conn = Arc::new(tokio::sync::RwLock::new(proto_conn));

    // Subscribe to gateway events
    let mut event_rx = state.event_tx.subscribe();

    // Command channel for cross-task communication
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<WsCommand>(256);

    // Split socket into sender/receiver so recv().await doesn't block sends.
    // WebSocket implements Stream + Sink; StreamExt::split yields independent halves.
    let (mut ws_sender, mut ws_receiver): (
        SplitSink<WebSocket, Message>,
        SplitStream<WebSocket>,
    ) = StreamExt::split(socket);
    let conn_send = conn.clone();

    // ── Send Task: pushes events and responses to client ─────────────────────
    let send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Ok(event) = event_rx.recv() => {
                    let conn_guard = conn_send.read().await;
                    // Only send if handshaked
                    if !conn_guard.handshaked {
                        continue;
                    }

                    // Filter by subscription
                    let should_send = match &event {
                        GatewayEvent::AgentResponse { session_id, .. }
                        | GatewayEvent::ToolCalling { session_id, .. }
                        | GatewayEvent::ToolResult { session_id, .. }
                        | GatewayEvent::Completed { session_id, .. }
                        | GatewayEvent::ProcessingError { session_id, .. }
                        | GatewayEvent::Thinking { session_id, .. } => {
                            conn_guard.is_subscribed(session_id)
                        }
                        _ => true, // Global events always sent
                    };

                    if !should_send {
                        continue;
                    }
                    drop(conn_guard);

                    // Convert GatewayEvent -> WsEvent
                    if let Some((event_name, payload)) = gateway_event_to_ws(&event) {
                        let seq = {
                            let mut cg = conn_send.write().await;
                            cg.next_seq()
                        };
                        let ws_event = WsEvent::new(event_name, payload, seq);
                        if let Ok(text) = serde_json::to_string(&ws_event) {
                            if ws_sender.send(Message::Text(text)).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                Some(cmd) = cmd_rx.recv() => {
                    match cmd {
                        WsCommand::SendResponse(text) | WsCommand::SendEvent(text) => {
                            if ws_sender.send(Message::Text(text)).await.is_err() {
                                break;
                            }
                        }
                        WsCommand::Subscribe(ids) => {
                            let mut cg = conn_send.write().await;
                            for id in ids {
                                if !cg.subscriptions.contains(&id) {
                                    cg.subscriptions.push(id);
                                }
                            }
                        }
                        WsCommand::Unsubscribe(ids) => {
                            let mut cg = conn_send.write().await;
                            cg.subscriptions.retain(|s| !ids.contains(s));
                        }
                        WsCommand::SubscribeAll => {
                            let mut cg = conn_send.write().await;
                            cg.subscriptions.clear();
                        }
                    }
                }
                else => break,
            }
        }
    });

    // ── Receive Task: processes client messages ──────────────────────────────
    let recv_task = tokio::spawn(async move {
        // Phase 1: Wait for connect handshake
        let handshake_ok = loop {
            let msg = ws_receiver.next().await;

            match msg {
                Some(Ok(Message::Text(text))) => {
                    let conn_id = conn.read().await.conn_id.clone();
                    debug!("[{}] Received: {}", conn_id, text);

                    match serde_json::from_str::<WsRequest>(&text) {
                        Ok(req) => {
                            if req.method == "connect" {
                                let res = handle_connect(
                                    &req, &conn, &state, &auth_mode, &cmd_tx
                                ).await;
                                let res_text = serde_json::to_string(&res).unwrap_or_default();
                                let _ = cmd_tx.send(WsCommand::SendResponse(res_text)).await;

                                if res.ok {
                                    conn.write().await.handshaked = true;
                                    break true;
                                } else {
                                    // Auth failed, give client a moment then close
                                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                                    break false;
                                }
                            } else {
                                // First message must be connect
                                let res = WsResponse::err(
                                    req.id,
                                    "INVALID_REQUEST",
                                    "First message must be connect"
                                );
                                let res_text = serde_json::to_string(&res).unwrap_or_default();
                                let _ = cmd_tx.send(WsCommand::SendResponse(res_text)).await;
                                break false;
                            }
                        }
                        Err(e) => {
                            let conn_id = conn.read().await.conn_id.clone();
                            warn!("[{}] Failed to parse frame: {}", conn_id, e);
                            break false;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None => break false,
                Some(Err(_)) => break false,
                _ => {}
            }
        };

        if !handshake_ok {
            let conn_id = conn.read().await.conn_id.clone();
            info!("[{}] Handshake failed, disconnecting", conn_id);
            return;
        }

        // Phase 2: Normal operation loop
        loop {
            let msg = ws_receiver.next().await;

            match msg {
                Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Ping(data))) => {
                    let conn_id = conn.read().await.conn_id.clone();
                    debug!("[{}] Received ping: {:?}", conn_id, data);
                }
                Some(Ok(Message::Text(text))) => {
                    let conn_id = conn.read().await.conn_id.clone();
                    debug!("[{}] Received: {}", conn_id, text);

                    match serde_json::from_str::<WsRequest>(&text) {
                        Ok(req) => {
                            let res = dispatch_method(
                                &req, &conn, &state, &cmd_tx
                            ).await;
                            let res_text = serde_json::to_string(&res).unwrap_or_default();
                            let _ = cmd_tx.send(WsCommand::SendResponse(res_text)).await;
                        }
                        Err(e) => {
                            let conn_id = conn.read().await.conn_id.clone();
                            warn!("[{}] Failed to parse request: {}", conn_id, e);
                        }
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
                None => break,
            }
        }

        let conn_id = conn.read().await.conn_id.clone();
        info!("[{}] WebSocket disconnected", conn_id);
    });

    // Wait for either task to complete
    tokio::select! {
        _ = send_task => {}
        _ = recv_task => {}
    }

    info!("[{}] WebSocket session ended", conn_id);
}

// ── Method Dispatch ───────────────────────────────────────────────────────────

async fn dispatch_method(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    cmd_tx: &mpsc::Sender<WsCommand>,
) -> WsResponse {
    // Check scope
    let scopes = conn.read().await.scopes.clone();
    if let Some(required) = method_scope(&req.method) {
        if !scopes_allow(&scopes, &req.method) {
            return error_forbidden(&req.id, required);
        }
    }

    match req.method.as_str() {
        "ping" => handle_ping(req),
        "connect" => WsResponse::err(&req.id, "INVALID_REQUEST", "connect can only be sent as first message"
        ),
        "chat.send" => handle_chat_send(req, conn, state).await,
        "chat.history" => handle_chat_history(req, conn, state).await,
        "chat.abort" => handle_chat_abort(req, conn, state).await,
        "sessions.list" => handle_sessions_list(req, state).await,
        "sessions.create" => handle_sessions_create(req, conn, state).await,
        "sessions.delete" => handle_sessions_delete(req, conn, state).await,
        "sessions.reset" => handle_sessions_reset(req, conn, state).await,
        "sessions.subscribe" => {
            handle_sessions_subscribe(req, conn, cmd_tx).await
        }
        "sessions.unsubscribe" => {
            handle_sessions_unsubscribe(req, conn, cmd_tx).await
        }
        "agents.list" => handle_agents_list(req, state).await,
        "agents.get" => handle_agents_get(req, state).await,
        "health" => handle_health(req, state).await,
        "system.presence" => handle_system_presence(req).await,
        "commands.list" => WsResponse::ok(&req.id, crate::gateway::commands::handle_commands_list()),
        "commands.execute" => {
            crate::gateway::commands::handle_commands_execute(req, conn, state).await
        }
        // Legacy commands (still supported during migration)
        "subscribe" => handle_legacy_subscribe(req, conn, cmd_tx).await,
        "unsubscribe" => handle_legacy_unsubscribe(req, conn, cmd_tx).await,
        "subscribe_all" => {
            conn.write().await.subscriptions.clear();
            WsResponse::ok(&req.id, serde_json::json!({"status": "subscribed_all"}))
        }
        _ => error_method_not_found(&req.id, &req.method),
    }
}

// ── Handshake Handler ─────────────────────────────────────────────────────────

async fn handle_connect(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    auth_mode: &crate::gateway::protocol::AuthMode,
    _cmd_tx: &mpsc::Sender<WsCommand>,
) -> WsResponse {
    // Parse params
    let params = match req.params.as_ref() {
        Some(p) => match serde_json::from_value::<ConnectParams>(p.clone()) {
            Ok(c) => c,
            Err(e) => {
                return error_invalid_request(&req.id, format!("Invalid connect params: {}", e));
            }
        },
        None => {
            return error_invalid_request(&req.id, "Missing connect params");
        }
    };

    // Protocol version check
    if params.protocol_version < PROTOCOL_VERSION_MIN || params.protocol_version > PROTOCOL_VERSION {
        return error_version_mismatch(&req.id);
    }

    // Auth resolution
    let (user_id, granted_scopes) = match auth_mode {
        crate::gateway::protocol::AuthMode::None => {
            // No auth required, grant default scopes
            (Some(UserId::new("anonymous")), DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect())
        }
        crate::gateway::protocol::AuthMode::Token => {
            resolve_token_auth(req, state, &params, conn).await
        }
        crate::gateway::protocol::AuthMode::Device => {
            return handle_device_auth(req, state, &params, conn).await;
        }
        crate::gateway::protocol::AuthMode::Tailscale => {
            // Tailscale auth is handled at the network layer
            (Some(UserId::new("tailscale")), DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect())
        }
    };

    finalize_hello_ok(req, conn, &params, user_id, granted_scopes).await
}

async fn finalize_hello_ok(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    params: &ConnectParams,
    user_id: Option<UserId>,
    granted_scopes: Vec<String>,
) -> WsResponse {
    let conn_id = {
        let mut cg = conn.write().await;
        if let Some(ref client) = params.client {
            cg.client = Some(client.clone());
        }
        cg.user_id = user_id.clone();
        cg.scopes = granted_scopes.clone();
        cg.conn_id.clone()
    };

    let channel = {
        let cg = conn.read().await;
        cg.client.as_ref().map(|c| c.id.clone()).unwrap_or_else(|| "ws".to_string())
    };
    let user_str = user_id.as_ref().map(|u| u.0.as_str()).unwrap_or("anonymous");
    let session_key = format!("{}:{}", channel, user_str);

    let payload = HelloOkPayload {
        protocol_version: PROTOCOL_VERSION,
        session_key,
        features: vec![
            "chat".to_string(),
            "sessions".to_string(),
            "agents".to_string(),
            "tools".to_string(),
        ],
        scopes_granted: granted_scopes,
        server: ServerInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            conn_id: conn_id.clone(),
        },
    };

    let scopes = conn.read().await.scopes.clone();
    info!(
        "[{}] Handshake complete: user={:?} scopes={:?}",
        conn_id, user_id, scopes
    );

    WsResponse::ok(&req.id, payload)
}

async fn resolve_token_auth(
    _req: &WsRequest,
    state: &Arc<GatewayState>,
    params: &ConnectParams,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
) -> (Option<UserId>, Vec<String>) {
    let token = params
        .auth
        .as_ref()
        .and_then(|a| a.token.as_ref())
        .cloned();

    if let Some(token_str) = token {
        if let Some(session) = state.auth_manager.validate_session(&token_str).await {
            let scopes = if session.scopes.is_empty() {
                DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
            } else {
                session.scopes.clone()
            };
            return (Some(session.user_id), scopes);
        }
    }

    // Try shared token from config
    let config = state.config.read().await;
    if let Some(shared_token) = &config.security.shared_token {
        if let Some(auth) = &params.auth {
            if let Some(token) = &auth.token {
                if token == shared_token {
                    let scopes = if params.scopes.is_empty() {
                        DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
                    } else {
                        params.scopes.clone()
                    };
                    return (Some(UserId::new("shared")), scopes);
                }
            }
        }
    }

    (None, Vec::new())
}

async fn handle_device_auth(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    params: &ConnectParams,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
) -> WsResponse {
    use crate::gateway::GatewayEvent;
    use crate::security::device_pairing::DeviceAccessResult;

    // 1. If client already has a device token, validate it
    if let Some(token) = params.auth.as_ref().and_then(|a| a.token.as_ref()) {
        if let Some(device_id) = state.device_pairing_store.validate_token(token).await {
            let scopes = if params.scopes.is_empty() {
                DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
            } else {
                params.scopes.clone()
            };
            return finalize_hello_ok(req, conn, params, Some(UserId::new(&device_id)), scopes).await;
        }
    }

    // 2. Device identity is required
    let device = match &params.device {
        Some(d) => d,
        None => {
            return error_invalid_request(&req.id, "Device auth requires device.id");
        }
    };

    // 3. Request pairing access
    let result = state
        .device_pairing_store
        .request_access(
            &device.id,
            None,
            device.public_key.as_deref(),
        )
        .await;

    match result {
        DeviceAccessResult::Authorized { token: _ } => {
            // Already authorized (shouldn't happen after validate_token, but handle it)
            let scopes = if params.scopes.is_empty() {
                DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
            } else {
                params.scopes.clone()
            };
            finalize_hello_ok(req, conn, params, Some(UserId::new(&device.id)), scopes).await
        }
        DeviceAccessResult::PairingRequired { code } => {
            // Broadcast to admin clients
            let _ = state.event_tx.send(GatewayEvent::DevicePairRequested {
                device_id: device.id.clone(),
                code: code.clone(),
                display_name: None,
            });
            error_invalid_request(
                &req.id,
                format!("Device pairing required. Use 'manta device approve {}' to approve.", code),
            )
        }
        DeviceAccessResult::AlreadyPending { code } => {
            error_invalid_request(
                &req.id,
                format!("Device pairing pending. Code: {}. Wait for admin approval.", code),
            )
        }
        DeviceAccessResult::RateLimited => error_rate_limited(&req.id),
    }
}

// ── Method Handlers ───────────────────────────────────────────────────────────

fn handle_ping(req: &WsRequest) -> WsResponse {
    WsResponse::ok(&req.id, serde_json::json!({}))
}

async fn handle_chat_send(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct ChatSendParams {
        message: String,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        agent_id: Option<String>,
    }

    let params: ChatSendParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    // Derive or use session ID
    let (session_id, is_new_session) = if let Some(sid) = params.session_id {
        (sid, false)
    } else {
        let cg = conn.read().await;
        let channel = cg.client.as_ref().map(|c| c.id.as_str()).unwrap_or("ws");
        let user = cg.user_id.as_ref().map(|u| u.0.as_str()).unwrap_or("anonymous");
        (format!("{}:{}", channel, user), true)
    };

    // Build IncomingMessage
    let user_id = {
        let cg = conn.read().await;
        cg.user_id.as_ref().map(|u| u.0.clone()).unwrap_or_else(|| "anonymous".to_string())
    };

    // Save user message to persistent session history
    if let Some(ref store) = state.session_store {
        if let Err(e) = store.append_message(&session_id, "user", &params.message, None, None, None).await {
            tracing::warn!("Failed to save user message to session history: {}", e);
        }
    }

    let incoming = crate::channels::IncomingMessage::new(
        user_id.clone(),
        session_id.clone(),
        params.message,
    )
    .with_provenance(crate::channels::InputProvenance::ExternalUser {
        channel: "web".to_string(),
        is_direct: true,
    });

    // Route through inbound pipeline
    match state.inbound_pipeline.process(incoming).await {
        Some(routed) => {
            // Subscribe to this session automatically
            let mut cg = conn.write().await;
            if !cg.subscriptions.contains(&session_id) {
                cg.subscriptions.push(session_id.clone());
            }
            drop(cg);

            // Notify clients if this is a newly derived session
            if is_new_session {
                let _ = state.event_tx.send(crate::gateway::GatewayEvent::SessionCreated {
                    session_id: session_id.clone(),
                    agent_id: routed.agent_id.clone(),
                    user_id: user_id.clone(),
                });
            }

            WsResponse::ok(&req.id, serde_json::json!({
                "status": "accepted",
                "session_id": session_id,
                "agent_id": routed.agent_id,
            }))
        }
        None => {
            WsResponse::ok(&req.id, serde_json::json!({
                "status": "queued",
                "session_id": session_id,
            }))
        }
    }
}

async fn handle_chat_history(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct HistoryParams {
        session_id: String,
        #[serde(default = "default_limit")]
        limit: usize,
    }

    fn default_limit() -> usize { 50 }

    let params: HistoryParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let messages = if let Some(ref store) = state.session_store {
        match store.get_messages(&params.session_id, params.limit as i64, None).await {
            Ok(rows) => rows
                .into_iter()
                .map(|(id, role, content, reasoning, tool_calls_json, dt)| {
                    let tool_calls: Option<serde_json::Value> = tool_calls_json
                        .and_then(|json| serde_json::from_str(&json).ok());
                    serde_json::json!({
                        "id": format!("msg_{}", id),
                        "role": role,
                        "content": content,
                        "reasoning_content": reasoning,
                        "tool_calls": tool_calls,
                        "timestamp": dt.timestamp(),
                    })
                })
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    WsResponse::ok(&req.id, serde_json::json!({
        "session_id": params.session_id,
        "messages": messages,
    }))
}

async fn handle_chat_abort(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    _state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct AbortParams {
        session_id: String,
    }

    let params: AbortParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    // TODO: implement abort via ACP
    WsResponse::ok(&req.id, serde_json::json!({
        "status": "abort_requested",
        "session_id": params.session_id,
    }))
}

async fn handle_sessions_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let sessions = {
        let mgr = state.session_manager.read().await;
        mgr.list_sessions()
    };

    WsResponse::ok(&req.id, serde_json::json!({ "sessions": sessions }))
}

async fn handle_sessions_create(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let cg = conn.read().await;
    let channel = cg.client.as_ref().map(|c| c.id.as_str()).unwrap_or("ws").to_string();
    let user = cg.user_id.as_ref().map(|u| u.0.clone()).unwrap_or_else(|| "anonymous".to_string());
    drop(cg);

    #[derive(Debug, Deserialize)]
    struct CreateParams {
        #[serde(default)]
        session_id: Option<String>,
    }

    let params: CreateParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let session_id = params
        .session_id
        .unwrap_or_else(|| format!("{}:{}", channel, user));

    {
        let mut mgr = state.session_manager.write().await;
        mgr.create_session(session_id.clone());
    }

    WsResponse::ok(&req.id, serde_json::json!({
        "session_id": session_id,
        "status": "created",
    }))
}

async fn handle_sessions_delete(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct DeleteParams {
        session_id: String,
    }

    let params: DeleteParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    {
        let mut mgr = state.session_manager.write().await;
        mgr.terminate_session(&params.session_id);
    }

    WsResponse::ok(&req.id, serde_json::json!({ "status": "deleted" }))
}

async fn handle_sessions_reset(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    _state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct ResetParams {
        session_id: String,
    }

    let _params: ResetParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    // TODO: implement session reset
    WsResponse::ok(&req.id, serde_json::json!({ "status": "reset" }))
}

async fn handle_sessions_subscribe(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    cmd_tx: &mpsc::Sender<WsCommand>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct SubscribeParams {
        session_ids: Vec<String>,
    }

    let params: SubscribeParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let _ = cmd_tx.send(WsCommand::Subscribe(params.session_ids.clone())).await;

    WsResponse::ok(&req.id, serde_json::json!({
        "subscribed": params.session_ids,
    }))
}

async fn handle_sessions_unsubscribe(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    cmd_tx: &mpsc::Sender<WsCommand>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct UnsubscribeParams {
        session_ids: Vec<String>,
    }

    let params: UnsubscribeParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let _ = cmd_tx.send(WsCommand::Unsubscribe(params.session_ids.clone())).await;

    WsResponse::ok(&req.id, serde_json::json!({
        "unsubscribed": params.session_ids,
    }))
}

async fn handle_agents_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let agents = {
        let agents = state.agents.read().await;
        agents.keys().cloned().collect::<Vec<_>>()
    };

    WsResponse::ok(&req.id, serde_json::json!({ "agents": agents }))
}

async fn handle_agents_get(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct GetParams {
        agent_id: String,
    }

    let params: GetParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let agent = {
        let agents = state.agents.read().await;
        agents.get(&params.agent_id).cloned()
    };

    match agent {
        Some(handle) => WsResponse::ok(&req.id, serde_json::json!({
            "agent_id": params.agent_id,
            "busy": handle.busy,
        })),
        None => error_agent_not_found(&req.id),
    }
}

async fn handle_health(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let agent_count = {
        let agents = state.agents.read().await;
        agents.len()
    };

    WsResponse::ok(&req.id, serde_json::json!({
        "status": "healthy",
        "agents": agent_count,
        "protocol_version": PROTOCOL_VERSION,
    }))
}

async fn handle_system_presence(
    req: &WsRequest,
) -> WsResponse {
    // Simplified presence info
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "online": true,
        }),
    )
}

// ── Legacy Compatibility ──────────────────────────────────────────────────────

async fn handle_legacy_subscribe(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    cmd_tx: &mpsc::Sender<WsCommand>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct LegacySubscribeParams {
        session_ids: Vec<String>,
    }

    let params: LegacySubscribeParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let _ = cmd_tx.send(WsCommand::Subscribe(params.session_ids)).await;

    WsResponse::ok(&req.id, serde_json::json!({ "status": "subscribed" }))
}

async fn handle_legacy_unsubscribe(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    cmd_tx: &mpsc::Sender<WsCommand>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct LegacyUnsubscribeParams {
        session_ids: Vec<String>,
    }

    let params: LegacyUnsubscribeParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let _ = cmd_tx.send(WsCommand::Unsubscribe(params.session_ids)).await;

    WsResponse::ok(&req.id, serde_json::json!({ "status": "unsubscribed" }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_params<T: serde::de::DeserializeOwned>(req: &WsRequest) -> Result<T, WsResponse> {
    match &req.params {
        Some(p) => match serde_json::from_value::<T>(p.clone()) {
            Ok(v) => Ok(v),
            Err(e) => Err(error_invalid_request(
                &req.id, format!("Invalid params: {}", e)
            )),
        },
        None => Err(error_invalid_request(&req.id, "Missing params"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_request_deserialize() {
        let json = r#"{"type":"req","id":"r1","method":"chat.send","params":{"message":"hello"}}"#;
        let req: WsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, "r1");
        assert_eq!(req.method, "chat.send");
        assert!(req.params.is_some());
    }

    #[test]
    fn test_ws_response_serialize() {
        let res = WsResponse::ok("r1", serde_json::json!({"status": "ok"}));
        let json = serde_json::to_string(&res).unwrap();
        assert!(json.contains("\"type\":\"res\""));
        assert!(json.contains("\"ok\":true"));
    }

    #[test]
    fn test_ws_event_serialize() {
        let evt = WsEvent::new("chat.delta", serde_json::json!({"content": "hi"}), 1);
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("\"type\":\"event\""));
        assert!(json.contains("\"event\":\"chat.delta\""));
    }

    #[test]
    fn test_connect_params_deserialize() {
        let json = r#"{"protocol_version":1,"client":{"id":"web","version":"1.0"},"auth":{"token":"abc"},"scopes":["chat","read"]}"#;
        let params: ConnectParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.protocol_version, 1);
        assert_eq!(params.client.as_ref().unwrap().id, "web");
        assert_eq!(params.scopes, vec!["chat", "read"]);
    }
}
