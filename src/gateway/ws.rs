//! WebSocket Protocol for Manta Gateway
//!
//! Implements the WebSocket-native RPC protocol (docs/protocol.md).
//!
//! Protocol flow:
//!   1. Client opens WebSocket to /ws
//!   2. Server validates auth (session cookie or shared token) - rejects with 401 if missing
//!   3. Server accepts WebSocket connection
//!   4. Client sends `connect` req as first frame
//!   5. Server validates auth + protocol version, replies `hello-ok`
//!   6. Client sends method calls (e.g. `chat.send`), server replies `res`
//!   7. Server pushes events (`chat.delta`, `tool.calling`, etc.) asynchronously

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    middleware::Next,
    response::IntoResponse,
};
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use base64::Engine;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::gateway::protocol::*;
use crate::gateway::{GatewayEvent, GatewayState};
use crate::security::UserId;

/// Query parameters for WebSocket upgrade
#[derive(Debug, Deserialize)]
pub struct WsConnectQuery {
    /// Optional: pre-subscribe to a session on connect
    pub session_id: Option<String>,
    /// Optional: client identifier hint
    pub client: Option<String>,
    /// Optional: authentication token (alternative to cookie/Bearer)
    pub token: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum WsCommand {
    SendResponse(String),
    SendEvent(String),
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
    SubscribeAll,
}

/// Pre-validated WebSocket authentication result, injected via Extension
/// by the `ws_auth_middleware` BEFORE the WebSocket upgrade happens.
#[derive(Debug, Clone)]
pub struct WsAuthResult {
    pub user_id: UserId,
    pub scopes: Vec<String>,
}

/// Middleware: validate WebSocket upgrade credentials before proceeding.
///
/// Runs BEFORE the WebSocket upgrade. When auth_mode is not "none", rejects
/// with 401 if no valid session cookie, shared token, or query token is found.
pub async fn ws_auth_middleware(
    State(state): State<Arc<GatewayState>>,
    mut req: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    // Check auth_mode — if "none", allow anonymous connections
    let auth_mode = {
        let config = state.config.read().await;
        config.security.auth_mode
    };

    if matches!(auth_mode, crate::gateway::protocol::AuthMode::None) {
        // Allow anonymous access
        req.extensions_mut().insert(WsAuthResult {
            user_id: UserId::new("anonymous"),
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
        });
        return next.run(req).await;
    }

    // Extract optional token from query parameter
    let query_token = req
        .uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .find(|p| p.starts_with("token="))
                .and_then(|p| urlencoding::decode(&p["token=".len()..]).ok())
                .map(|s| s.to_string())
        });

    let auth_result = validate_ws_upgrade_request(&state, req.headers(), query_token.as_deref()).await;
    match auth_result {
        Ok(result) => {
            req.extensions_mut().insert(result);
            next.run(req).await
        }
        Err(resp) => resp,
    }
}

/// Validate WebSocket upgrade request credentials BEFORE the handshake.
async fn validate_ws_upgrade_request(
    state: &Arc<GatewayState>,
    headers: &axum::http::HeaderMap,
    query_token: Option<&str>,
) -> Result<WsAuthResult, axum::response::Response> {
    // 1. Try session cookie
    let cookie_config = crate::gateway::auth::SessionCookieConfig::default();
    if let Some(token) =
        crate::gateway::auth::extract_session_cookie_from_headers(headers, &cookie_config.name)
    {
        if let Some(session) = state.auth_manager.validate_session(&token).await {
            let scopes = if session.scopes.is_empty() {
                DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
            } else {
                session.scopes.clone()
            };
            return Ok(WsAuthResult {
                user_id: session.user_id,
                scopes,
            });
        }
    }

    // 2. Try Bearer token from Authorization header
    let token_from_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer ").map(String::from));

    // Check against auth_manager (for Bearer session tokens)
    if let Some(ref tok) = token_from_header {
        if let Some(session) = state.auth_manager.validate_session(tok).await {
            let scopes = if session.scopes.is_empty() {
                DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
            } else {
                session.scopes.clone()
            };
            return Ok(WsAuthResult {
                user_id: session.user_id,
                scopes,
            });
        }
    }

    // 3. Check against shared_token in config
    let config = state.config.read().await;
    if let Some(shared_token) = &config.security.shared_token {
        // Check Bearer header token
        if let Some(ref tok) = token_from_header {
            if tok == shared_token {
                return Ok(WsAuthResult {
                    user_id: UserId::new("shared"),
                    scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
                });
            }
        }
        // Check query parameter token
        if let Some(qt) = query_token {
            if qt == shared_token {
                return Ok(WsAuthResult {
                    user_id: UserId::new("shared"),
                    scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
                });
            }
        }
    }

    warn!("WebSocket upgrade rejected: no valid credentials");
    let resp = axum::http::Response::builder()
        .status(axum::http::StatusCode::UNAUTHORIZED)
        .header(axum::http::header::WWW_AUTHENTICATE, "Bearer, Cookie")
        .body(axum::body::Body::from(
            "Unauthorized: valid session cookie or API token required",
        ))
        .unwrap();
    Err(resp)
}

/// Handler: WebSocket upgrade.
///
/// Credentials are validated by the `ws_auth_middleware` BEFORE this handler
/// is reached. If we get here, auth is already verified.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<WsConnectQuery>,
    axum::Extension(auth_result): axum::Extension<WsAuthResult>,
) -> impl IntoResponse {
    let auth_mode = {
        let config = state.config.read().await;
        config.security.auth_mode
    };

    ws.on_upgrade(move |socket| handle_websocket(socket, state, query, auth_mode, auth_result))
}

async fn handle_websocket(
    socket: WebSocket,
    state: Arc<GatewayState>,
    query: WsConnectQuery,
    auth_mode: crate::gateway::protocol::AuthMode,
    auth_result: WsAuthResult,
) {
    let conn_id = Uuid::new_v4().to_string();
    info!("[{}] WebSocket connected", conn_id);

    let mut proto_conn = ProtocolConnection::new(conn_id.clone());
    if let Some(sid) = query.session_id {
        proto_conn.subscriptions.push(sid);
    }
    let conn = Arc::new(tokio::sync::RwLock::new(proto_conn));

    let mut event_rx = state.event_tx.subscribe();
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<WsCommand>(256);

    let (mut ws_sender, mut ws_receiver): (SplitSink<WebSocket, Message>, SplitStream<WebSocket>) =
        StreamExt::split(socket);
    let conn_send = conn.clone();

    let send_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Ok(event) = event_rx.recv() => {
                    let conn_guard = conn_send.read().await;
                    if !conn_guard.handshaked {
                        continue;
                    }

                    let should_send = match &event {
                        GatewayEvent::AgentResponse { session_id, .. }
                        | GatewayEvent::ToolCalling { session_id, .. }
                        | GatewayEvent::ToolResult { session_id, .. }
                        | GatewayEvent::Completed { session_id, .. }
                        | GatewayEvent::ProcessingError { session_id, .. }
                        | GatewayEvent::Thinking { session_id, .. } => {
                            conn_guard.is_subscribed(session_id)
                        }
                        _ => true,
                    };

                    if !should_send {
                        continue;
                    }
                    drop(conn_guard);

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

    let recv_task = tokio::spawn(async move {
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
                                    &req,
                                    &conn,
                                    &state,
                                    &auth_mode,
                                    &cmd_tx,
                                    &auth_result,
                                )
                                .await;
                                let res_text = serde_json::to_string(&res).unwrap_or_default();
                                let _ = cmd_tx.send(WsCommand::SendResponse(res_text)).await;

                                if res.ok {
                                    conn.write().await.handshaked = true;
                                    break true;
                                } else {
                                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                                    break false;
                                }
                            } else {
                                let res = WsResponse::err(
                                    req.id,
                                    "INVALID_REQUEST",
                                    "First message must be connect",
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
                            let res = dispatch_method(&req, &conn, &state, &cmd_tx).await;
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

    tokio::select! {
        _ = send_task => {}
        _ = recv_task => {}
    }

    info!("[{}] WebSocket session ended", conn_id);
}

async fn dispatch_method(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    cmd_tx: &mpsc::Sender<WsCommand>,
) -> WsResponse {
    let scopes = conn.read().await.scopes.clone();
    if let Some(required) = method_scope(&req.method) {
        if !scopes_allow(&scopes, &req.method) {
            if req.method == "commands.execute" {
                if let Some(ref params_val) = req.params {
                    if let Ok(params) =
                        serde_json::from_value::<serde_json::Value>(params_val.clone())
                    {
                        if let Some(session_id) = params.get("session_id").and_then(|v| v.as_str())
                        {
                            let command = params
                                .get("command")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");
                            let user_text = format!("/{}", command);
                            let error_text =
                                format!("Command error: Missing required scope: {}", required);
                            if let Some(ref store) = state.session_store {
                                let _ = store
                                    .append_message(
                                        session_id, "user", &user_text, None, None, None, None,
                                        None,
                                    )
                                    .await;
                                let _ = store
                                    .append_message(
                                        session_id,
                                        "assistant",
                                        &error_text,
                                        None,
                                        None,
                                        None,
                                        None,
                                        None,
                                    )
                                    .await;
                            }
                        }
                    }
                }
            }
            return error_forbidden(&req.id, required);
        }
    }

    match req.method.as_str() {
        "ping" => handle_ping(req),
        "connect" => {
            WsResponse::err(&req.id, "INVALID_REQUEST", "connect can only be sent as first message")
        }
        "chat.send" => handle_chat_send(req, conn, state).await,
        "chat.history" => handle_chat_history(req, conn, state).await,
        "chat.abort" => handle_chat_abort(req, conn, state).await,
        "sessions.list" => handle_sessions_list(req, state).await,
        "sessions.create" => handle_sessions_create(req, conn, state).await,
        "sessions.delete" => handle_sessions_delete(req, conn, state).await,
        "sessions.reset" => handle_sessions_reset(req, conn, state).await,
        "sessions.subscribe" => handle_sessions_subscribe(req, conn, cmd_tx).await,
        "sessions.unsubscribe" => handle_sessions_unsubscribe(req, conn, cmd_tx).await,
        "agents.list" => handle_agents_list(req, state).await,
        "agents.get" => handle_agents_get(req, state).await,
        "agents.registry" => handle_agents_registry(req, state).await,
        "health" => handle_health(req, state).await,
        "system.presence" => handle_system_presence(req).await,
        "commands.list" => {
            WsResponse::ok(&req.id, crate::gateway::commands::handle_commands_list())
        }
        "commands.execute" => {
            crate::gateway::commands::handle_commands_execute(req, conn, state).await
        }
        "config.get" => handle_config_get(req, state).await,
        "config.set" => handle_config_set(req, state).await,
        "models.list" => handle_models_list(req, state).await,
        "models.presets" => handle_models_presets(req, state).await,
        "models.add" => handle_models_add(req, state).await,
        "models.remove" => handle_models_remove(req, state).await,
        "models.set_default" => handle_models_set_default(req, state).await,
        "mcp.list" => handle_mcp_list(req, state).await,
        "mcp.add" => handle_mcp_add(req, state).await,
        "mcp.remove" => handle_mcp_remove(req, state).await,
        "mcp.connect" => handle_mcp_connect(req, state).await,
        "mcp.disconnect" => handle_mcp_disconnect(req, state).await,
        "cron.list" => handle_cron_list(req, state).await,
        "skills.list" => handle_skills_list(req, state).await,
        "skills.install" => handle_skills_install(req, state).await,
        "logs.subscribe" => handle_logs_subscribe(req, conn, state, cmd_tx).await,
        "logs.unsubscribe" => handle_logs_unsubscribe(req, conn).await,
        "acp.list" => handle_acp_list(req, state).await,
        "acp.spawn" => handle_acp_spawn(req, conn, state).await,
        "acp.terminate" => handle_acp_terminate(req, state).await,
        "acp.message" => handle_acp_message(req, state).await,
        "acp.status" => handle_acp_status(req, state).await,
        "acp.pause" => handle_acp_pause(req, state).await,
        "acp.resume" => handle_acp_resume(req, state).await,
        "acp.step" => handle_acp_step(req, state).await,
        "acp.cancel" => handle_acp_cancel(req, state).await,
        "acp.tree" => handle_acp_tree(req, state).await,
        "acp.execute.session" => handle_acp_execute_session(req, state).await,
        "acp.execute.run" => handle_acp_execute_run(req, state).await,
        "subscribe" => handle_legacy_subscribe(req, conn, cmd_tx).await,
        "unsubscribe" => handle_legacy_unsubscribe(req, conn, cmd_tx).await,
        "subscribe_all" => {
            conn.write().await.subscriptions.clear();
            WsResponse::ok(&req.id, serde_json::json!({"status": "subscribed_all"}))
        }
        _ => error_method_not_found(&req.id, &req.method),
    }
}

async fn handle_connect(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    auth_mode: &crate::gateway::protocol::AuthMode,
    _cmd_tx: &mpsc::Sender<WsCommand>,
    pre_validated_auth: &WsAuthResult,
) -> WsResponse {
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

    if params.protocol_version < PROTOCOL_VERSION_MIN || params.protocol_version > PROTOCOL_VERSION
    {
        return error_version_mismatch(&req.id);
    }

    let (user_id, granted_scopes) = match auth_mode {
        crate::gateway::protocol::AuthMode::None => {
            // WebSocket upgrade already validated credentials at the HTTP layer.
            // Use the pre-validated identity instead of granting anonymous access.
            let mut scopes = pre_validated_auth.scopes.clone();
            for s in &params.scopes {
                if crate::gateway::protocol::ALL_SCOPES.contains(&s.as_str()) && !scopes.contains(s)
                {
                    scopes.push(s.clone());
                }
            }
            (Some(pre_validated_auth.user_id.clone()), scopes)
        }
        crate::gateway::protocol::AuthMode::Token => {
            resolve_token_auth(req, state, &params, conn).await
        }
        crate::gateway::protocol::AuthMode::Device => {
            return handle_device_auth(req, state, &params, conn).await;
        }
        crate::gateway::protocol::AuthMode::Tailscale => (
            Some(UserId::new("tailscale")),
            DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
        ),
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
        cg.handshaked = true;
        cg.conn_id.clone()
    };

    let channel = {
        let cg = conn.read().await;
        cg.client
            .as_ref()
            .map(|c| c.id.clone())
            .unwrap_or_else(|| "ws".to_string())
    };
    let user_str = user_id
        .as_ref()
        .map(|u| u.0.as_str())
        .unwrap_or("anonymous");
    let session_key = format!("{}:{}", channel, user_str);

    let payload = HelloOkPayload {
        protocol_version: PROTOCOL_VERSION,
        session_key,
        features: vec![
            "chat".to_string(),
            "sessions".to_string(),
            "agents".to_string(),
            "tools".to_string(),
            "acp".to_string(),
        ],
        scopes_granted: granted_scopes,
        server: ServerInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            conn_id: conn_id.clone(),
        },
    };

    let scopes = conn.read().await.scopes.clone();
    info!("[{}] Handshake complete: user={:?} scopes={:?}", conn_id, user_id, scopes);

    WsResponse::ok(&req.id, payload)
}

async fn resolve_token_auth(
    _req: &WsRequest,
    state: &Arc<GatewayState>,
    params: &ConnectParams,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
) -> (Option<UserId>, Vec<String>) {
    let token = params.auth.as_ref().and_then(|a| a.token.as_ref()).cloned();

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

    if let Some(token) = params.auth.as_ref().and_then(|a| a.token.as_ref()) {
        if let Some(device_id) = state.device_pairing_store.validate_token(token).await {
            let scopes = if params.scopes.is_empty() {
                DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
            } else {
                params.scopes.clone()
            };
            return finalize_hello_ok(req, conn, params, Some(UserId::new(&device_id)), scopes)
                .await;
        }
    }

    let device = match &params.device {
        Some(d) => d,
        None => {
            return error_invalid_request(&req.id, "Device auth requires device.id");
        }
    };

    let result = state
        .device_pairing_store
        .request_access(&device.id, None, device.public_key.as_deref())
        .await;

    match result {
        DeviceAccessResult::Authorized { token: _ } => {
            let scopes = if params.scopes.is_empty() {
                DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
            } else {
                params.scopes.clone()
            };
            finalize_hello_ok(req, conn, params, Some(UserId::new(&device.id)), scopes).await
        }
        DeviceAccessResult::PairingRequired { code } => {
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
        DeviceAccessResult::AlreadyPending { code } => error_invalid_request(
            &req.id,
            format!("Device pairing pending. Code: {}. Wait for admin approval.", code),
        ),
        DeviceAccessResult::RateLimited => error_rate_limited(&req.id),
    }
}

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
        #[serde(alias = "content")]
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

    let (session_id, is_new_session) = if let Some(sid) = params.session_id {
        (sid, false)
    } else {
        let cg = conn.read().await;
        let channel = cg.client.as_ref().map(|c| c.id.as_str()).unwrap_or("ws");
        let user = cg
            .user_id
            .as_ref()
            .map(|u| u.0.as_str())
            .unwrap_or("anonymous");
        (format!("{}:{}", channel, user), true)
    };

    let user_id = {
        let cg = conn.read().await;
        cg.user_id
            .as_ref()
            .map(|u| u.0.clone())
            .unwrap_or_else(|| "anonymous".to_string())
    };

    let mut should_name = false;
    if let Some(ref store) = state.session_store {
        if let Err(e) = store
            .append_message(&session_id, "user", &params.message, None, None, None, None, None)
            .await
        {
            tracing::warn!("Failed to save user message to session history: {}", e);
        }
        if let Ok(ps) = store.load_session(&session_id).await {
            if ps.as_ref().map(|m| m.message_count).unwrap_or(0) <= 1 {
                if let Ok(existing) = store.get_session_name(&session_id).await {
                    if existing.is_none() {
                        should_name = true;
                    }
                }
            }
        }
    }

    if should_name {
        let store = state.session_store.clone();
        let sid = session_id.clone();
        let msg = params.message.clone();
        let trimmed = msg.trim();
        let name = trimmed
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ");
        let name = if name.len() > 40 {
            format!("{}...", &name[..40])
        } else if name.is_empty() {
            "New Session".to_string()
        } else {
            name
        };
        tokio::spawn(async move {
            if let Some(ref s) = store {
                if let Err(e) = s.set_session_name(&sid, &name).await {
                    tracing::warn!("Failed to save session name for {}: {}", sid, e);
                } else {
                    tracing::info!("Session {} named: '{}'", sid, name);
                }
            }
        });
    }

    if let Some(ref store) = state.session_store {
        if let Ok(Some(ps)) = store.load_session(&session_id).await {
            if let Some(ref bound_agent) = ps.metadata.bound_agent_id {
                let route = crate::inbound::RouteResult {
                    agent_id: bound_agent.clone(),
                    workspace_id: None,
                    created_binding: false,
                };
                state.agent_router.bind_session(&session_id, &route).await;
            }
        }
    }

    if let Some(agent_id) = params.agent_id {
        let route = crate::inbound::RouteResult {
            agent_id,
            workspace_id: None,
            created_binding: false,
        };
        state.agent_router.bind_session(&session_id, &route).await;
    }

    // ── Smart name-based routing: "小王，xxx" -> route to secretary-xiaowang ──
    let mut final_message = params.message.clone();
    {
        let registry = state.agent_registry.read().await;
        // Try to extract a name prefix like "小王，" or "小王：" from the message.
        let trimmed = final_message.trim_start();
        if let Some((first_word, rest)) = trimmed.split_once(['，', ',', '：', ':', ' ', '\t']) {
            let name = first_word.trim();
            if !name.is_empty() {
                if let Some((personality, _matched_alias)) = registry.find_by_alias(name) {
                    let agent_id = personality.id.clone();
                    info!("Smart-routing session {} to agent '{}' (matched name: '{}' in message)", session_id, agent_id, name);
                    let route = crate::inbound::RouteResult {
                        agent_id: agent_id.clone(),
                        workspace_id: None,
                        created_binding: true,
                    };
                    state.agent_router.bind_session(&session_id, &route).await;
                    // Strip the greeting prefix so the agent sees only the task.
                    final_message = rest.trim_start().to_string();
                }
            }
        }
    }

    let incoming =
        crate::channels::IncomingMessage::new(user_id.clone(), session_id.clone(), final_message)
            .with_provenance(crate::channels::InputProvenance::ExternalUser {
                channel: "web".to_string(),
                is_direct: true,
            });

    let routed = state.inbound_pipeline.process(incoming).await;

    if let Some(ref _r) = routed {
        let mut cg = conn.write().await;
        if !cg.subscriptions.contains(&session_id) {
            cg.subscriptions.push(session_id.clone());
        }
        drop(cg);
    }

    if is_new_session {
        let _ = state
            .event_tx
            .send(crate::gateway::GatewayEvent::SessionCreated {
                session_id: session_id.clone(),
                agent_id: routed
                    .as_ref()
                    .map(|r| r.agent_id.clone())
                    .unwrap_or_default(),
                user_id: user_id.clone(),
            });
    }

    match routed {
        Some(routed) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "status": "accepted",
                "session_id": session_id,
                "agent_id": routed.agent_id,
            }),
        ),
        None => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "status": "queued",
                "session_id": session_id,
            }),
        ),
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

    fn default_limit() -> usize {
        50
    }

    let params: HistoryParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let messages = if let Some(ref store) = state.session_store {
        match store
            .get_messages(&params.session_id, params.limit as i64, None)
            .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(
                    |(
                        id,
                        role,
                        content,
                        reasoning,
                        tool_calls_json,
                        dt,
                        _transcript_id,
                        _run_id,
                    )| {
                        let tool_calls: Option<serde_json::Value> =
                            tool_calls_json.and_then(|json| serde_json::from_str(&json).ok());
                        serde_json::json!({
                            "id": format!("msg_{}", id),
                            "role": role,
                            "content": content,
                            "reasoning_content": reasoning,
                            "tool_calls": tool_calls,
                            "timestamp": dt.timestamp(),
                        })
                    },
                )
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "session_id": params.session_id,
            "messages": messages,
        }),
    )
}

async fn handle_chat_abort(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct AbortParams {
        session_id: String,
    }

    let params: AbortParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    state.acp.cancel(params.session_id.clone()).await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "aborted",
            "session_id": params.session_id,
        }),
    )
}

async fn handle_sessions_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let sessions: Vec<serde_json::Value> = if let Some(ref store) = state.session_store {
        match store.find_sessions(None, None, None, false).await {
            Ok(rows) => rows
                .into_iter()
                .map(|meta| {
                    let name = meta.name.unwrap_or_else(|| {
                        if meta.message_count == 0 {
                            "New Session".to_string()
                        } else {
                            meta.last_activity.format("%b %d %H:%M").to_string()
                        }
                    });
                    serde_json::json!({
                        "session_id": meta.session_id,
                        "name": name,
                        "agent_id": meta.agent_id,
                        "channel": meta.channel,
                        "message_count": meta.message_count,
                        "last_activity": meta.last_activity.to_rfc3339(),
                        "is_active": meta.is_active,
                        "created_at": meta.created_at.to_rfc3339(),
                    })
                })
                .collect(),
            Err(e) => {
                tracing::warn!("Failed to list sessions from store: {}", e);
                Vec::new()
            }
        }
    } else {
        let mgr = state.session_manager.read().await;
        mgr.list_sessions()
            .await
            .into_iter()
            .map(|id| serde_json::json!({ "session_id": id, "name": id }))
            .collect()
    };

    WsResponse::ok(&req.id, serde_json::json!({ "sessions": sessions }))
}

async fn handle_sessions_create(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let cg = conn.read().await;
    let channel = cg
        .client
        .as_ref()
        .map(|c| c.id.as_str())
        .unwrap_or("ws")
        .to_string();
    let user = cg
        .user_id
        .as_ref()
        .map(|u| u.0.clone())
        .unwrap_or_else(|| "anonymous".to_string());
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

    if let Some(ref store) = state.session_store {
        let metadata = crate::agent::session_store::SessionMetadata::new(&session_id, "", "", "");
        let _ = store.save_session(&session_id, &metadata, "{}").await;
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "session_id": session_id,
            "status": "created",
        }),
    )
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

    if let Some(ref store) = state.session_store {
        let _ = store.delete_session(&params.session_id).await;
    }

    WsResponse::ok(&req.id, serde_json::json!({ "status": "deleted" }))
}

async fn handle_sessions_reset(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct ResetParams {
        session_id: String,
    }

    let params: ResetParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    state.acp.cancel(params.session_id.clone()).await;

    if let Some(ref store) = state.session_store {
        let _ = store.delete_session(&params.session_id).await;
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "reset",
            "session_id": params.session_id,
        }),
    )
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

    let _ = cmd_tx
        .send(WsCommand::Subscribe(params.session_ids.clone()))
        .await;

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "subscribed": params.session_ids,
        }),
    )
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

    let _ = cmd_tx
        .send(WsCommand::Unsubscribe(params.session_ids.clone()))
        .await;

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "unsubscribed": params.session_ids,
        }),
    )
}

async fn handle_agents_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agents = {
        let agents = state.agents.read().await;
        agents.keys().cloned().collect::<Vec<_>>()
    };

    WsResponse::ok(&req.id, serde_json::json!({ "agents": agents }))
}

async fn handle_agents_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
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

    let personality = {
        let registry = state.agent_registry.read().await;
        registry.get(&params.agent_id).cloned()
    };

    match agent {
        Some(handle) => {
            let cfg = &handle.config;
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "agent_id": params.agent_id,
                    "busy": handle.busy,
                    "status": if handle.busy { "busy" } else { "idle" },
                    "config": {
                        "temperature": cfg.temperature,
                        "max_tokens": cfg.max_tokens,
                        "max_turns": cfg.max_turns,
                        "max_concurrent_tools": cfg.max_concurrent_tools,
                        "workspace_only": cfg.workspace_only,
                        "compaction_model": cfg.compaction_model,
                        "system_prompt": cfg.system_prompt,
                    },
                    "personality": personality.map(|p| serde_json::json!({
                        "display_name": p.display_name(),
                        "is_valid": p.is_valid,
                        "has_heartbeat": !p.heartbeat.is_empty(),
                        "has_soul": !p.soul.is_empty(),
                        "has_identity": !p.identity.is_empty(),
                        "has_memory": !p.memory.is_empty(),
                    })),
                }),
            )
        }
        None => {
            // Agent not spawned but may have a personality on disk
            if let Some(p) = personality {
                WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "agent_id": params.agent_id,
                        "busy": false,
                        "status": "stopped",
                        "config": null,
                        "personality": {
                            "display_name": p.display_name(),
                            "is_valid": p.is_valid,
                            "has_heartbeat": !p.heartbeat.is_empty(),
                            "has_soul": !p.soul.is_empty(),
                            "has_identity": !p.identity.is_empty(),
                            "has_memory": !p.memory.is_empty(),
                        },
                    }),
                )
            } else {
                error_agent_not_found(&req.id)
            }
        }
    }
}

async fn handle_agents_registry(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let registry = state.agent_registry.read().await;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entries: Vec<serde_json::Value> = Vec::new();

    // 1. Registry-discovered agents from disk
    for id in registry.list() {
        if let Some(p) = registry.get(&id) {
            seen.insert(id.clone());
            entries.push(serde_json::json!({
                "id": p.id,
                "display_name": p.display_name(),
                "is_valid": p.is_valid,
                "has_heartbeat": !p.heartbeat.is_empty(),
            }));
        }
    }

    // 2. Runtime-spawned agents not in registry (e.g. default)
    {
        let agents = state.agents.read().await;
        for id in agents.keys() {
            if !seen.contains(id) {
                entries.push(serde_json::json!({
                    "id": id,
                    "display_name": id.as_str(),
                    "is_valid": true,
                    "has_heartbeat": false,
                }));
            }
        }
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({ "agents": entries, "count": entries.len() }),
    )
}

async fn handle_health(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agent_count = {
        let agents = state.agents.read().await;
        agents.len()
    };

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "healthy",
            "agents": agent_count,
            "protocol_version": PROTOCOL_VERSION,
        }),
    )
}

async fn handle_system_presence(req: &WsRequest) -> WsResponse {
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "online": true,
        }),
    )
}

async fn handle_acp_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
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

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "sessions": sessions,
            "count": sessions.len(),
        }),
    )
}

async fn handle_acp_spawn(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    use crate::acp::{AcpSessionId, SpawnMode, SubagentConfig, ThreadBinding};
    use crate::channels::IncomingMessage;
    use crate::security::runtime_audit::AuditEventType;
    use crate::security::RateLimitResult;

    #[derive(Debug, Deserialize)]
    struct SpawnParams {
        task: String,
        #[serde(default = "default_acp_mode")]
        mode: String,
        #[serde(default)]
        agent_type: String,
    }

    fn default_acp_mode() -> String {
        "run".to_string()
    }

    let params: SpawnParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let actor = {
        let cg = conn.read().await;
        cg.user_id
            .as_ref()
            .map(|u| u.0.clone())
            .unwrap_or_else(|| "anonymous".to_string())
    };

    let rate_result = state
        .rate_limiter
        .check_with_cost(&crate::security::UserId::new(format!("acp:spawn:{}", actor)), 1.0)
        .await;
    if !rate_result.is_allowed() {
        let retry = match rate_result {
            RateLimitResult::Denied { retry_after_secs } => retry_after_secs,
            _ => 60,
        };
        return WsResponse::err(
            &req.id,
            "RATE_LIMITED",
            format!("Rate limit exceeded for ACP spawn. Retry after {}s", retry),
        );
    }

    let session_id = AcpSessionId::new();
    let parent_id = actor.clone();

    let mode = match params.mode.as_str() {
        "session" => SpawnMode::Session,
        _ => SpawnMode::Run,
    };

    let agent_type = if params.agent_type.is_empty() {
        "default".to_string()
    } else {
        params.agent_type.clone()
    };
    let config = SubagentConfig {
        agent_type: agent_type.clone(),
        mode,
        thread_binding: ThreadBinding::Auto,
        system_prompt: None,
        max_tokens: None,
        temperature: None,
        tools: vec![],
        context: None,
        timeout_seconds: Some(300),
        retry_on_crash: false,
        max_crash_retries: 3,
    };

    match state
        .acp
        .spawn_subagent(session_id.clone(), parent_id.clone(), config)
        .await
    {
        Ok(handle) => {
            let subagent_id = handle.id.clone();

            state
                .audit_log
                .log(
                    AuditEventType::AcpSpawn,
                    &actor,
                    &subagent_id,
                    true,
                    format!("Spawned subagent via WebSocket (mode: {:?})", handle.mode),
                    Some(serde_json::json!({
                        "session_id": session_id.to_string(),
                        "parent_id": parent_id,
                        "agent_type": agent_type,
                    })),
                )
                .await;

            let message = IncomingMessage::new(actor.clone(), session_id.to_string(), params.task);

            match state.acp.send_message(&subagent_id, message).await {
                Ok(response) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "subagent_id": subagent_id,
                        "session_id": session_id.to_string(),
                        "mode": format!("{:?}", handle.mode),
                        "response": response,
                    }),
                ),
                Err(e) => {
                    let _ = state.acp.shutdown_subagent(&subagent_id).await;
                    WsResponse::err(
                        &req.id,
                        "SPAWN_FAILED",
                        format!("Subagent failed to process task: {}", e),
                    )
                }
            }
        }
        Err(e) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpSpawn,
                    &actor,
                    "",
                    false,
                    format!("Failed to spawn subagent: {}", e),
                    None,
                )
                .await;
            WsResponse::err(&req.id, "SPAWN_FAILED", format!("Failed to spawn subagent: {}", e))
        }
    }
}

async fn handle_acp_terminate(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    use crate::acp::AcpSessionId;
    use crate::security::runtime_audit::AuditEventType;

    #[derive(Debug, Deserialize)]
    struct TerminateParams {
        session_id: String,
    }

    let params: TerminateParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let session_id = AcpSessionId(params.session_id.clone());
    match state.acp.terminate_session(&session_id).await {
        Ok(count) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpTerminate,
                    "ws-user",
                    &params.session_id,
                    true,
                    format!("Terminated {} subagents in session {}", count, params.session_id),
                    Some(serde_json::json!({ "terminated_count": count })),
                )
                .await;
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "terminated_count": count,
                    "session_id": params.session_id,
                }),
            )
        }
        Err(e) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpTerminate,
                    "ws-user",
                    &params.session_id,
                    false,
                    format!("Failed to terminate session: {}", e),
                    None,
                )
                .await;
            WsResponse::err(
                &req.id,
                "TERMINATE_FAILED",
                format!("Failed to terminate session: {}", e),
            )
        }
    }
}

async fn handle_acp_message(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    use crate::acp::AcpSessionId;
    use crate::channels::IncomingMessage;
    use crate::security::runtime_audit::AuditEventType;

    #[derive(Debug, Deserialize)]
    struct MessageParams {
        session_id: String,
        message: String,
    }

    let params: MessageParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let session_id = AcpSessionId(params.session_id.clone());
    let subagents = state.acp.list_session_subagents(&session_id).await;

    if subagents.is_empty() {
        return WsResponse::err(&req.id, "NO_ACTIVE_SUBAGENTS", "No active subagents in session");
    }

    let subagent = &subagents[0];
    let message =
        IncomingMessage::new("ws-user".to_string(), session_id.to_string(), params.message);

    match state.acp.send_message(&subagent.id, message).await {
        Ok(response) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpMessage,
                    "ws-user",
                    &params.session_id,
                    true,
                    format!(
                        "Message sent to subagent {} in session {}",
                        subagent.id, params.session_id
                    ),
                    Some(serde_json::json!({
                        "subagent_id": subagent.id,
                        "session_id": params.session_id,
                    })),
                )
                .await;
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "subagent_id": subagent.id,
                    "session_id": session_id.to_string(),
                    "response": response,
                }),
            )
        }
        Err(e) => {
            state
                .audit_log
                .log(
                    AuditEventType::AcpMessage,
                    "ws-user",
                    &params.session_id,
                    false,
                    format!("Failed to send message: {}", e),
                    None,
                )
                .await;
            WsResponse::err(&req.id, "MESSAGE_FAILED", format!("Failed to send message: {}", e))
        }
    }
}

async fn handle_acp_status(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct StatusParams {
        session_id: String,
    }

    let params: StatusParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    match state.acp.get_status(params.session_id.clone()).await {
        Some(status) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "session_id": status.session_id,
                "runtime_state": format!("{}", status.runtime_state),
                "mode": format!("{:?}", status.mode),
                "current_iteration": status.current_iteration,
                "max_iterations": status.max_iterations,
            }),
        ),
        None => WsResponse::err(&req.id, "SESSION_NOT_FOUND", "Session not found"),
    }
}

async fn handle_acp_pause(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct ControlParams {
        session_id: String,
    }

    let params: ControlParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    state.acp.pause(params.session_id.clone()).await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "session_id": params.session_id,
            "action": "pause",
            "status": "requested",
        }),
    )
}

async fn handle_acp_resume(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct ControlParams {
        session_id: String,
    }

    let params: ControlParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    state.acp.resume(params.session_id.clone()).await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "session_id": params.session_id,
            "action": "resume",
            "status": "requested",
        }),
    )
}

async fn handle_acp_step(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct ControlParams {
        session_id: String,
    }

    let params: ControlParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    state.acp.step(params.session_id.clone()).await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "session_id": params.session_id,
            "action": "step",
            "status": "requested",
        }),
    )
}

async fn handle_acp_cancel(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct ControlParams {
        session_id: String,
    }

    let params: ControlParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    state.acp.cancel(params.session_id.clone()).await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "session_id": params.session_id,
            "action": "cancel",
            "status": "requested",
        }),
    )
}

async fn handle_acp_tree(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    use crate::acp::AcpSessionId;

    #[derive(Debug, Deserialize)]
    struct TreeParams {
        session_id: String,
    }

    let params: TreeParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let session_id = AcpSessionId(params.session_id.clone());
    let tree = state.acp.get_subagent_tree(&session_id).await;

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "session_id": params.session_id,
            "tree": tree,
        }),
    )
}

async fn handle_acp_execute_session(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct ExecuteParams {
        message: String,
        user_id: String,
        #[serde(default)]
        agent_id: Option<String>,
        #[serde(default)]
        max_iterations: Option<usize>,
    }

    let params: ExecuteParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let agent_id = params.agent_id.unwrap_or_else(|| "default".to_string());
    let agents = state.agents.read().await;
    let agent_handle = match agents.get(&agent_id) {
        Some(h) => h.clone(),
        None => {
            return WsResponse::err(
                &req.id,
                "AGENT_NOT_FOUND",
                format!("Agent '{}' not found", agent_id),
            );
        }
    };
    drop(agents);

    let session_id = uuid::Uuid::new_v4().to_string();
    let incoming = crate::channels::IncomingMessage::new(
        params.user_id.clone(),
        session_id.clone(),
        params.message,
    );

    match state
        .acp
        .execute_session_with_max_iterations(agent_handle.agent, incoming, params.max_iterations)
        .await
    {
        Ok(outgoing) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "session_id": session_id,
                "mode": "session",
                "response": outgoing.content,
                "usage": outgoing.usage,
            }),
        ),
        Err(e) => WsResponse::err(&req.id, "EXECUTE_FAILED", format!("Execution failed: {}", e)),
    }
}

async fn handle_acp_execute_run(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct ExecuteParams {
        message: String,
        user_id: String,
        #[serde(default)]
        agent_id: Option<String>,
        #[serde(default)]
        max_iterations: Option<usize>,
    }

    let params: ExecuteParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let agent_id = params.agent_id.unwrap_or_else(|| "default".to_string());
    let agents = state.agents.read().await;
    let agent_handle = match agents.get(&agent_id) {
        Some(h) => h.clone(),
        None => {
            return WsResponse::err(
                &req.id,
                "AGENT_NOT_FOUND",
                format!("Agent '{}' not found", agent_id),
            );
        }
    };
    drop(agents);

    let session_id = uuid::Uuid::new_v4().to_string();
    let incoming = crate::channels::IncomingMessage::new(
        params.user_id.clone(),
        session_id.clone(),
        params.message,
    );

    match state
        .acp
        .execute_run_with_max_iterations(agent_handle.agent, incoming, params.max_iterations)
        .await
    {
        Ok(outgoing) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "session_id": session_id,
                "mode": "run",
                "response": outgoing.content,
                "usage": outgoing.usage,
            }),
        ),
        Err(e) => WsResponse::err(&req.id, "EXECUTE_FAILED", format!("Execution failed: {}", e)),
    }
}

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

    let _ = cmd_tx
        .send(WsCommand::Unsubscribe(params.session_ids))
        .await;

    WsResponse::ok(&req.id, serde_json::json!({ "status": "unsubscribed" }))
}

async fn handle_config_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let config = state.config.read().await;
    let heartbeat = &config.heartbeat;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "model": config.model,
            "model_provider": config.model_provider,
            "default_agent": {
                "temperature": config.default_agent.temperature,
                "max_tokens": config.default_agent.max_tokens,
                "max_turns": config.default_agent.max_turns,
                "max_concurrent_tools": config.default_agent.max_concurrent_tools,
                "system_prompt": config.default_agent.system_prompt,
                "workspace_only": config.default_agent.workspace_only,
            },
            "heartbeat": {
                "enabled": heartbeat.enabled,
                "interval_seconds": heartbeat.interval_seconds,
                "active_hours_start": heartbeat.active_hours_start,
                "active_hours_end": heartbeat.active_hours_end,
                "max_consecutive_idle": heartbeat.max_consecutive_idle,
            },
            "channels": config.channels.iter().map(|(k, v)| {
                serde_json::json!({
                    "name": k,
                    "channel_type": format!("{:?}", v.channel_type).to_lowercase(),
                    "enabled": v.enabled,
                    "agent_id": v.agent_id,
                    "dm_policy": format!("{:?}", v.dm_policy).to_lowercase(),
                    "require_mention": v.require_mention,
                    "has_credentials": !v.credentials.is_empty(),
                })
            }).collect::<Vec<_>>(),
            "auth_mode": config.security.auth_mode,
        }),
    )
}

async fn handle_config_set(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct SetParams {
        path: String,
        value: serde_json::Value,
    }

    let params: SetParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let mut config = state.config.write().await;

    match params.path.as_str() {
        "model" => {
            if let Some(v) = params.value.as_str() {
                config.model = v.to_string();
                // Also update model router default alias
                if let Err(e) = state.model_router.switch_default_model(v).await {
                    return WsResponse::err(&req.id,
                        "CONFIG_ERROR",
                        format!("Failed to switch model: {}", e),
                    );
                }
            }
        }
        "model_provider" => {
            if let Some(v) = params.value.as_str() {
                config.model_provider = v.to_string();
            }
        }
        "default_agent.temperature" => {
            if let Some(v) = params.value.as_f64() {
                config.default_agent.temperature = v as f32;
            }
        }
        "default_agent.max_tokens" => {
            if let Some(v) = params.value.as_u64() {
                config.default_agent.max_tokens = v as u32;
            }
        }
        "default_agent.max_turns" => {
            config.default_agent.max_turns = params.value.as_u64().map(|v| v as usize);
        }
        "default_agent.max_concurrent_tools" => {
            if let Some(v) = params.value.as_u64() {
                config.default_agent.max_concurrent_tools = v as usize;
            }
        }
        "default_agent.system_prompt" => {
            if let Some(v) = params.value.as_str() {
                config.default_agent.system_prompt = v.to_string();
            }
        }
        "default_agent.workspace_only" => {
            if let Some(v) = params.value.as_bool() {
                config.default_agent.workspace_only = v;
            }
        }
        "heartbeat.enabled" => {
            if let Some(v) = params.value.as_bool() {
                config.heartbeat.enabled = v;
            }
        }
        "heartbeat.interval_seconds" => {
            if let Some(v) = params.value.as_u64() {
                config.heartbeat.interval_seconds = v;
            }
        }
        "heartbeat.active_hours_start" => {
            if let Some(v) = params.value.as_str() {
                config.heartbeat.active_hours_start = v.to_string();
            }
        }
        "heartbeat.active_hours_end" => {
            if let Some(v) = params.value.as_str() {
                config.heartbeat.active_hours_end = v.to_string();
            }
        }
        "heartbeat.max_consecutive_idle" => {
            if let Some(v) = params.value.as_u64() {
                config.heartbeat.max_consecutive_idle = v as u32;
            }
        }
        "channels.add" => {
            #[derive(Debug, Deserialize)]
            struct ChannelAddPayload {
                name: String,
                channel_type: String,
                enabled: Option<bool>,
                agent_id: Option<String>,
                credentials: Option<HashMap<String, String>>,
            }
            let payload: ChannelAddPayload = match serde_json::from_value(params.value) {
                Ok(p) => p,
                Err(e) => return WsResponse::err(&req.id, "INVALID_PARAMS", e.to_string()),
            };
            let channel_type = match payload.channel_type.as_str() {
                "telegram" => crate::channels::ChannelType::Telegram,
                "discord" => crate::channels::ChannelType::Discord,
                "slack" => crate::channels::ChannelType::Slack,
                "whatsapp" => crate::channels::ChannelType::Whatsapp,
                "qq" => crate::channels::ChannelType::Qq,
                "feishu" => crate::channels::ChannelType::Feishu,
                "signal" => crate::channels::ChannelType::Signal,
                "imessage" => crate::channels::ChannelType::Imessage,
                "webchat" => crate::channels::ChannelType::Webchat,
                "websocket" => crate::channels::ChannelType::Websocket,
                "web_terminal" => crate::channels::ChannelType::WebTerminal,
                other => return WsResponse::err(&req.id, "INVALID_CHANNEL_TYPE", format!("Unknown channel type: {}", other)),
            };
            let mut ch = crate::gateway::ChannelConfig::new(channel_type);
            if let Some(v) = payload.enabled { ch.enabled = v; }
            if let Some(v) = payload.agent_id { ch.agent_id = Some(v); }
            if let Some(v) = payload.credentials { ch.credentials = v; }
            config.channels.insert(payload.name.clone(), ch);
        }
        "channels.update" => {
            #[derive(Debug, Deserialize)]
            struct ChannelUpdatePayload {
                name: String,
                enabled: Option<bool>,
                agent_id: Option<String>,
                credentials: Option<HashMap<String, String>>,
            }
            let payload: ChannelUpdatePayload = match serde_json::from_value(params.value) {
                Ok(p) => p,
                Err(e) => return WsResponse::err(&req.id, "INVALID_PARAMS", e.to_string()),
            };
            match config.channels.get_mut(&payload.name) {
                Some(ch) => {
                    if let Some(v) = payload.enabled { ch.enabled = v; }
                    if let Some(v) = payload.agent_id { ch.agent_id = Some(v); }
                    if let Some(v) = payload.credentials { ch.credentials = v; }
                }
                None => return WsResponse::err(&req.id, "CHANNEL_NOT_FOUND", format!("Channel '{}' not found", payload.name)),
            }
        }
        "channels.remove" => {
            if let Some(name) = params.value.as_str() {
                config.channels.remove(name);
            } else {
                return WsResponse::err(&req.id, "INVALID_PARAMS", "Expected channel name string");
            }
        }
        "channels.set_enabled" => {
            #[derive(Debug, Deserialize)]
            struct SetEnabledPayload {
                name: String,
                enabled: bool,
            }
            let payload: SetEnabledPayload = match serde_json::from_value(params.value) {
                Ok(p) => p,
                Err(e) => return WsResponse::err(&req.id, "INVALID_PARAMS", e.to_string()),
            };
            match config.channels.get_mut(&payload.name) {
                Some(ch) => ch.enabled = payload.enabled,
                None => return WsResponse::err(&req.id, "CHANNEL_NOT_FOUND", format!("Channel '{}' not found", payload.name)),
            }
        }
        _ => {
            return WsResponse::err(
                &req.id,
                "UNKNOWN_CONFIG_PATH",
                format!("Unknown config path: {}", params.path),
            );
        }
    }

    // Persist config to disk so changes survive restarts and trigger hot-reload
    drop(config);
    if let Some(config_path) = state.config_path.clone() {
        let config_guard = state.config.read().await;
        match toml::to_string_pretty(&*config_guard) {
            Ok(toml_str) => {
                if let Err(e) = tokio::fs::write(&config_path, toml_str).await {
                    return WsResponse::err(
                        &req.id,
                        "PERSIST_FAILED",
                        format!("Config updated in memory but failed to write to disk: {}", e),
                    );
                }
            }
            Err(e) => {
                return WsResponse::err(
                    &req.id,
                    "PERSIST_FAILED",
                    format!("Config updated in memory but TOML serialization failed: {}", e),
                );
            }
        }
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "updated",
            "path": params.path,
        }),
    )
}

async fn handle_models_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    // Build model list from aliases (always available) rather than catalog
    // which may be empty if initialize() was never called.
    let aliases = state.model_router.list_aliases().await;
    let entries: Vec<serde_json::Value> = {
        let config = state.model_router.config.read().await;
        aliases
            .iter()
            .filter_map(|name| config.aliases.get(name))
            .map(|alias| {
                serde_json::json!({
                    "id": alias.name,
                    "name": format!("{} ({})", alias.name, alias.model),
                    "provider": alias.provider,
                })
            })
            .collect()
    };
    let default_model = state.model_router.get_default_model().await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "models": entries,
            "default_model": default_model,
        }),
    )
}

async fn handle_models_presets(req: &WsRequest, _state: &Arc<GatewayState>) -> WsResponse {
    let presets = crate::model_router::provider_presets();
    let list: Vec<serde_json::Value> = presets
        .into_iter()
        .map(|(name, p)| {
            serde_json::json!({
                "name": name,
                "display_name": p.display_name,
                "base_url": p.default_base_url,
                "models": p.models,
            })
        })
        .collect();
    WsResponse::ok(&req.id, serde_json::json!({ "presets": list }))
}

async fn handle_models_add(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct ModelAddPayload {
        name: String,
        provider: String,
        model: String,
        api_key: Option<String>,
        base_url: Option<String>,
    }
    let payload: ModelAddPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let provider_name = payload.provider.clone();
    let presets = crate::model_router::provider_presets();
    let preset = presets.get(&provider_name);

    // If api_key provided, configure or update the provider
    if let Some(api_key) = payload.api_key.filter(|k| !k.is_empty()) {
        let (provider_type, base_url) = match preset {
            Some(p) => (
                p.protocol.clone(),
                payload.base_url.clone().or_else(|| p.default_base_url.clone()),
            ),
            None => (
                crate::model_router::ProviderType::Custom {
                    name: provider_name.clone(),
                },
                payload.base_url.clone(),
            ),
        };

        let provider_config = crate::model_router::ProviderConfig {
            provider_type,
            api_key: api_key.clone(),
            api_keys: Vec::new(),
            auth_profile: None,
            oauth: None,
            base_url,
            timeout: std::time::Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        // Update GatewayConfig providers
        {
            let mut config = state.config.write().await;
            config.providers.insert(provider_name.clone(), provider_config.clone());
        }

        // Register with model router
        if let Err(e) = state
            .model_router
            .add_provider(&provider_name, provider_config)
            .await
        {
            return WsResponse::err(
                &req.id,
                "PROVIDER_ERROR",
                format!("Failed to register provider: {}", e),
            );
        }
    }

    // Set alias
    let alias = crate::model_router::ModelAlias {
        name: payload.name.clone(),
        provider: provider_name,
        model: payload.model,
        temperature: None,
        max_tokens: None,
    };
    state.model_router.set_alias(alias).await;

    // Register in catalog for discovery
    let entry = crate::model_router::ModelCatalogEntry::new(
        payload.name.clone(),
        format!("{} ({})", payload.name, payload.name),
        payload.name.clone(),
    )
    .with_alias(payload.name.clone());
    state.model_router.model_catalog.register(entry).await;

    // Persist GatewayConfig to manta.toml
    if let Some(config_path) = state.config_path.clone() {
        let config_guard = state.config.read().await;
        match toml::to_string_pretty(&*config_guard) {
            Ok(toml_str) => {
                if let Err(e) = tokio::fs::write(&config_path, toml_str).await {
                    return WsResponse::err(
                        &req.id,
                        "PERSIST_FAILED",
                        format!("Model added but failed to write config: {}", e),
                    );
                }
            }
            Err(e) => {
                return WsResponse::err(
                    &req.id,
                    "PERSIST_FAILED",
                    format!("Model added but TOML serialization failed: {}", e),
                );
            }
        }
    }

    WsResponse::ok(&req.id, serde_json::json!({ "status": "added" }))
}

async fn handle_models_remove(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct RemovePayload {
        name: String,
    }
    let payload: RemovePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let removed = state.model_router.remove_alias(&payload.name).await;
    if removed {
        WsResponse::ok(&req.id, serde_json::json!({ "status": "removed" }))
    } else {
        WsResponse::err(&req.id, "MODEL_NOT_FOUND", format!("Model alias '{}' not found", payload.name))
    }
}

async fn handle_models_set_default(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct SetDefaultPayload {
        name: String,
    }
    let payload: SetDefaultPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state.model_router.switch_default_model(&payload.name).await {
        Ok(()) => WsResponse::ok(&req.id, serde_json::json!({ "status": "ok", "default_model": payload.name })),
        Err(e) => WsResponse::err(&req.id, "MODEL_NOT_FOUND", format!("{}", e)),
    }
}

async fn handle_mcp_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let connected = state.mcp_manager.list_servers().await;
    let config_guard = state.config.read().await;
    let servers: Vec<serde_json::Value> = config_guard
        .mcp
        .servers
        .iter()
        .map(|(id, cfg)| {
            serde_json::json!({
                "id": id,
                "transport": match cfg.transport {
                    crate::tools::mcp::McpTransport::Stdio => "stdio",
                    crate::tools::mcp::McpTransport::Sse => "sse",
                    crate::tools::mcp::McpTransport::StreamableHttp => "streamable_http",
                },
                "command": cfg.command,
                "args": cfg.args,
                "url": cfg.url,
                "auto_connect": cfg.auto_connect,
                "connected": connected.contains(id),
            })
        })
        .collect();
    WsResponse::ok(&req.id, serde_json::json!({ "servers": servers }))
}

async fn handle_mcp_add(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct McpAddPayload {
        id: String,
        transport: String,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        url: Option<String>,
        #[serde(default = "default_true")]
        auto_connect: bool,
    }
    fn default_true() -> bool { true }

    let payload: McpAddPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let transport = match payload.transport.as_str() {
        "sse" => crate::tools::mcp::McpTransport::Sse,
        "streamable_http" => crate::tools::mcp::McpTransport::StreamableHttp,
        _ => crate::tools::mcp::McpTransport::Stdio,
    };

    let config = crate::tools::mcp::McpServerConfig {
        transport,
        command: payload.command,
        args: payload.args,
        url: payload.url,
        auto_connect: payload.auto_connect,
        ..Default::default()
    };

    {
        let mut cfg = state.config.write().await;
        cfg.mcp.servers.insert(payload.id.clone(), config.clone());
    }

    if payload.auto_connect {
        if let Err(e) = state.mcp_manager.connect(&payload.id, config).await {
            return WsResponse::err(&req.id, "MCP_CONNECT_FAILED", format!("Saved config but failed to connect: {}", e));
        }
    }

    if let Err(e) = persist_config(state).await {
        return e;
    }

    WsResponse::ok(&req.id, serde_json::json!({ "status": "added", "id": payload.id }))
}

async fn handle_mcp_remove(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct McpRemovePayload {
        id: String,
    }
    let payload: McpRemovePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let _ = state.mcp_manager.disconnect(&payload.id).await;
    let prefix = format!("mcp__{}__", payload.id);
    state.tool_registry.deregister_prefix(&prefix);

    {
        let mut cfg = state.config.write().await;
        cfg.mcp.servers.remove(&payload.id);
    }

    if let Err(e) = persist_config(state).await {
        return e;
    }

    WsResponse::ok(&req.id, serde_json::json!({ "status": "removed", "id": payload.id }))
}

async fn handle_mcp_connect(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct McpConnectPayload {
        id: String,
    }
    let payload: McpConnectPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let config = {
        let cfg = state.config.read().await;
        match cfg.mcp.servers.get(&payload.id) {
            Some(c) => c.clone(),
            None => return WsResponse::err(&req.id, "MCP_NOT_FOUND", format!("MCP server '{}' not configured", payload.id)),
        }
    };

    match state.mcp_manager.connect(&payload.id, config).await {
        Ok(tools) => WsResponse::ok(&req.id, serde_json::json!({
            "status": "connected",
            "id": payload.id,
            "tool_count": tools.len(),
        })),
        Err(e) => WsResponse::err(&req.id, "MCP_CONNECT_FAILED", format!("{}", e)),
    }
}

async fn handle_mcp_disconnect(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct McpDisconnectPayload {
        id: String,
    }
    let payload: McpDisconnectPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    match state.mcp_manager.disconnect(&payload.id).await {
        Ok(()) => {
            let prefix = format!("mcp__{}__", payload.id);
            state.tool_registry.deregister_prefix(&prefix);
            WsResponse::ok(&req.id, serde_json::json!({ "status": "disconnected", "id": payload.id }))
        }
        Err(e) => WsResponse::err(&req.id, "MCP_DISCONNECT_FAILED", format!("{}", e)),
    }
}

async fn handle_cron_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let jobs = {
        let scheduler_opt = state.cron_scheduler.read().await;
        match scheduler_opt.as_ref() {
            Some(s) => s.lock().await.list_jobs().await,
            None => Vec::new(),
        }
    };
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "jobs": jobs,
            "count": jobs.len(),
        }),
    )
}

async fn handle_skills_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let skills = {
        let sm = state.skills_manager.read().await;
        sm.list_skills().await
    };
    let entries: Vec<_> = skills
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
                "version": s.version,
                "author": s.author,
                "triggers": s.triggers.iter().map(|t| {
                    serde_json::json!({
                        "type": format!("{:?}", t.trigger_type).to_lowercase(),
                        "pattern": t.pattern,
                    })
                }).collect::<Vec<_>>(),
                "depends_on": s.depends_on,
                "provides": s.provides,
                "chain": s.chain,
            })
        })
        .collect();
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "skills": entries,
            "count": entries.len(),
        }),
    )
}

async fn handle_skills_install(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct InstallPayload {
        name: String,
        #[serde(default)]
        content: Option<String>,
        #[serde(default, rename = "zip_base64")]
        zip_base64: Option<String>,
    }
    let payload: InstallPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let name = payload.name.trim();
    if name.is_empty() {
        return WsResponse::err(&req.id, "INVALID_PARAMS", "Skill name is required");
    }

    let skills_dir = crate::dirs::skills_dir();
    let skill_dir = skills_dir.join(name);
    if let Err(e) = tokio::fs::create_dir_all(&skill_dir).await {
        return WsResponse::err(&req.id, "INTERNAL_ERROR", format!("Failed to create skill directory: {}", e));
    }

    if let Some(zip_base64) = payload.zip_base64 {
        // Decode base64 ZIP
        let zip_bytes = match base64::engine::general_purpose::STANDARD.decode(&zip_base64) {
            Ok(b) => b,
            Err(e) => return WsResponse::err(&req.id, "INVALID_CONTENT", format!("Invalid base64: {}", e)),
        };

        let skill_dir_clone = skill_dir.clone();
        // Extract ZIP synchronously (ZipFile is not Send)
        let extract_task: tokio::task::JoinHandle<Result<Vec<(std::path::PathBuf, Vec<u8>)>, String>> = tokio::task::spawn_blocking(move || {
            let cursor = std::io::Cursor::new(zip_bytes);
            let mut archive = match zip::ZipArchive::new(cursor) {
                Ok(a) => a,
                Err(e) => return Err(format!("Invalid ZIP: {}", e)),
            };

            let mut files: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
            for i in 0..archive.len() {
                let mut file = match archive.by_index(i) {
                    Ok(f) => f,
                    Err(e) => return Err(format!("ZIP read error: {}", e)),
                };
                let outpath = match file.enclosed_name() {
                    Some(p) => skill_dir_clone.join(p),
                    None => continue,
                };
                if !file.is_dir() {
                    let mut contents = Vec::new();
                    if let Err(e) = std::io::Read::read_to_end(&mut file, &mut contents) {
                        return Err(format!("Failed to read ZIP entry: {}", e));
                    }
                    files.push((outpath, contents));
                }
            }
            Ok(files)
        });

        let files: Vec<(std::path::PathBuf, Vec<u8>)> = match extract_task.await {
            Ok(Ok(f)) => f,
            Ok(Err(msg)) => return WsResponse::err(&req.id, "INVALID_CONTENT", msg),
            Err(e) => return WsResponse::err(&req.id, "INTERNAL_ERROR", format!("ZIP extraction failed: {}", e)),
        };

        // Write extracted files
        for (outpath, contents) in files {
            if let Some(parent) = outpath.parent() {
                if !parent.exists() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        return WsResponse::err(&req.id, "INTERNAL_ERROR", format!("Failed to create directory: {}", e));
                    }
                }
            }
            if let Err(e) = tokio::fs::write(&outpath, &contents).await {
                return WsResponse::err(&req.id, "INTERNAL_ERROR", format!("Failed to write file: {}", e));
            }
        }

        // Validate SKILL.md exists and is valid
        let skill_md_path = skill_dir.join("SKILL.md");
        if !skill_md_path.exists() {
            let _ = tokio::fs::remove_dir_all(&skill_dir).await;
            return WsResponse::err(&req.id, "INVALID_CONTENT", "ZIP must contain SKILL.md at the root");
        }

        let skill_md_content = match tokio::fs::read_to_string(&skill_md_path).await {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&skill_dir).await;
                return WsResponse::err(&req.id, "INVALID_CONTENT", format!("Failed to read SKILL.md: {}", e));
            }
        };

        if let Err(e) = crate::skills::parse_skill_md(&skill_md_content) {
            let _ = tokio::fs::remove_dir_all(&skill_dir).await;
            return WsResponse::err(&req.id, "INVALID_CONTENT", format!("Invalid SKILL.md: {}", e));
        }
    } else if let Some(content) = payload.content {
        // Legacy single-file install
        if let Err(e) = crate::skills::parse_skill_md(&content) {
            return WsResponse::err(&req.id, "INVALID_CONTENT", format!("Invalid skill markdown: {}", e));
        }
        let skill_path = skill_dir.join("SKILL.md");
        if let Err(e) = tokio::fs::write(&skill_path, &content).await {
            return WsResponse::err(&req.id, "INTERNAL_ERROR", format!("Failed to write skill file: {}", e));
        }
    } else {
        return WsResponse::err(&req.id, "INVALID_PARAMS", "Either content or zip_base64 is required");
    }

    // Reload skills
    {
        let mut sm = state.skills_manager.write().await;
        if let Err(e) = sm.load_all().await {
            return WsResponse::err(&req.id, "INTERNAL_ERROR", format!("Failed to reload skills: {}", e));
        }
    }

    WsResponse::ok(&req.id, serde_json::json!({ "status": "installed", "name": name }))
}

#[allow(clippy::result_large_err)]
fn parse_params<T: serde::de::DeserializeOwned>(req: &WsRequest) -> Result<T, WsResponse> {
    match &req.params {
        Some(p) => match serde_json::from_value::<T>(p.clone()) {
            Ok(v) => Ok(v),
            Err(e) => Err(error_invalid_request(&req.id, format!("Invalid params: {}", e))),
        },
        None => Err(error_invalid_request(&req.id, "Missing params")),
    }
}

async fn handle_logs_subscribe(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    cmd_tx: &mpsc::Sender<WsCommand>,
) -> WsResponse {
    // Cancel any existing log subscription for this connection
    {
        let cg = conn.write().await;
        if let Some(ref tx) = cg.log_cancel_tx {
            let _ = tx.send(()).await;
        }
    }

    let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);
    {
        let mut cg = conn.write().await;
        cg.log_cancel_tx = Some(cancel_tx);
    }

    let log_tx = state.log_tx.clone();
    let cmd_tx = cmd_tx.clone();

    tokio::spawn(async move {
        // Subscribe to new log lines first to avoid missing any during file read
        let mut log_rx = log_tx.subscribe();

        // Send all historical log lines from the file
        let log_path = crate::logs::log_file_path();
        if log_path.exists() {
            if let Ok(file) = tokio::fs::File::open(&log_path).await {
                let reader = tokio::io::BufReader::new(file);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let event = WsEvent {
                        frame_type: "event",
                        event: "log.line".to_string(),
                        payload: serde_json::to_value(serde_json::json!({
                            "line": line,
                            "historical": true,
                        })).ok(),
                        seq: None,
                    };
                    if let Ok(text) = serde_json::to_string(&event) {
                        let _ = cmd_tx.send(WsCommand::SendEvent(text)).await;
                    }
                }
            }
        }

        // Forward new lines from the broadcast channel
        loop {
            tokio::select! {
                Ok(line) = log_rx.recv() => {
                    let event = WsEvent {
                        frame_type: "event",
                        event: "log.line".to_string(),
                        payload: serde_json::to_value(serde_json::json!({
                            "line": line,
                            "historical": false,
                        })).ok(),
                        seq: None,
                    };
                    if let Ok(text) = serde_json::to_string(&event) {
                        let _ = cmd_tx.send(WsCommand::SendEvent(text)).await;
                    }
                }
                _ = cancel_rx.recv() => {
                    break;
                }
            }
        }
    });

    WsResponse::ok(&req.id, serde_json::json!({ "status": "subscribed" }))
}

async fn handle_logs_unsubscribe(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
) -> WsResponse {
    let mut cg = conn.write().await;
    if let Some(ref tx) = cg.log_cancel_tx {
        let _ = tx.send(()).await;
        cg.log_cancel_tx = None;
    }
    WsResponse::ok(&req.id, serde_json::json!({ "status": "unsubscribed" }))
}

/// Persist GatewayConfig to manta.toml.
async fn persist_config(state: &Arc<GatewayState>) -> Result<(), WsResponse> {
    if let Some(config_path) = state.config_path.clone() {
        let config_guard = state.config.read().await;
        match toml::to_string_pretty(&*config_guard) {
            Ok(toml_str) => {
                if let Err(e) = tokio::fs::write(&config_path, toml_str).await {
                    return Err(WsResponse::err(
                        "persist",
                        "PERSIST_FAILED",
                        format!("Failed to write config: {}", e),
                    ));
                }
            }
            Err(e) => {
                return Err(WsResponse::err(
                    "persist",
                    "PERSIST_FAILED",
                    format!("TOML serialization failed: {}", e),
                ));
            }
        }
    }
    Ok(())
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
