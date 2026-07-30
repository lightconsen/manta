//! WebSocket Protocol for Syscity Gateway
//!
//! Implements the WebSocket-native RPC protocol (docs/protocol.md).
//!
//! Protocol flow:
//!   1. Client opens WebSocket to /ws
//!   2. Server validates auth (session cookie or shared token) - rejects with
//!      401 if missing
//!   3. Server accepts WebSocket connection
//!   4. Client sends `connect` req as first frame
//!   5. Server validates auth + protocol version, replies `hello-ok`
//!   6. Client sends method calls (e.g. `chat.send`), server replies `res`
//!   7. Server pushes events (`chat.delta`, `tool.calling`, etc.)
//!      asynchronously

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    middleware::Next,
    response::IntoResponse,
};
use base64::Engine;
use chrono::DateTime;
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use serde::Deserialize;
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;
use tracing::{debug, info, warn, Instrument};
use uuid::Uuid;

use crate::agent::session_store::AppendMessageParams;
use crate::core::context::RequestContext;
use crate::gateway::handlers::config::persist_config_atomic;
use crate::gateway::protocol::*;
use crate::gateway::{GatewayEvent, GatewayState};
use crate::providers::Message as ProviderMessage;
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
enum WsCommand {
    SendResponse(String),
    SendEvent(String),
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
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
    let query_token = req.uri().query().and_then(|q| {
        q.split('&')
            .find(|p| p.starts_with("token="))
            .and_then(|p| urlencoding::decode(&p["token=".len()..]).ok())
            .map(|s| s.to_string())
    });

    let auth_result =
        validate_ws_upgrade_request(&state, req.headers(), query_token.as_deref()).await;
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
        if let Some(session) = state.auth.manager.validate_session(&token).await {
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
        if let Some(session) = state.auth.manager.validate_session(tok).await {
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
        .unwrap_or_else(|_| {
            axum::http::Response::new(axum::body::Body::from(
                "Unauthorized: valid session cookie or API token required",
            ))
        });
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

    let mut event_rx = state.events.tx.subscribe();
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<WsCommand>(256);

    let (mut ws_sender, mut ws_receiver): (SplitSink<WebSocket, Message>, SplitStream<WebSocket>) =
        StreamExt::split(socket);
    let conn_send = conn.clone();

    let conn_task_prefix = format!("ws:conn:{}", conn_id);
    let task_registry = state.task_registry.clone();

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
                        | GatewayEvent::Thinking { session_id, .. }
                        | GatewayEvent::GoalProgress { session_id, .. } => {
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
                    }
                }
                else => break,
            }
        }
    });
    task_registry
        .insert_join(format!("{}:send", conn_task_prefix), send_task)
        .await;

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
                                if cmd_tx
                                    .send(WsCommand::SendResponse(res_text))
                                    .await
                                    .is_err()
                                {
                                    warn!("[{}] Failed to send handshake response", conn_id);
                                    break false;
                                }

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
                                if cmd_tx
                                    .send(WsCommand::SendResponse(res_text))
                                    .await
                                    .is_err()
                                {
                                    warn!("[{}] Failed to send invalid-request response", conn_id);
                                }
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
                            if cmd_tx
                                .send(WsCommand::SendResponse(res_text))
                                .await
                                .is_err()
                            {
                                warn!("[{}] Failed to send response, connection closed", conn_id);
                                break;
                            }
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
    task_registry
        .insert_join(format!("{}:recv", conn_task_prefix), recv_task)
        .await;

    let send_task_name = format!("{}:send", conn_task_prefix);
    let recv_task_name = format!("{}:recv", conn_task_prefix);

    let send_join = match task_registry.remove_join_or_abort(&send_task_name).await {
        Some(h) => h,
        None => {
            warn!("[{}] send task missing from registry", conn_id);
            return;
        }
    };
    let recv_join = match task_registry.remove_join_or_abort(&recv_task_name).await {
        Some(h) => h,
        None => {
            warn!("[{}] recv task missing from registry", conn_id);
            return;
        }
    };

    tokio::select! {
        _ = send_join => {}
        _ = recv_join => {}
    }

    task_registry.abort_matching(&conn_task_prefix).await;

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
                            if let Some(ref store) = state.agents.store {
                                if let Err(e) = store
                                    .append_message(&AppendMessageParams {
                                        session_id,
                                        role: "user",
                                        content: &user_text,
                                        ..Default::default()
                                    })
                                    .await
                                {
                                    warn!("Failed to append user command message: {}", e);
                                }
                                if let Err(e) = store
                                    .append_message(&AppendMessageParams {
                                        session_id,
                                        role: "assistant",
                                        content: &error_text,
                                        ..Default::default()
                                    })
                                    .await
                                {
                                    warn!("Failed to append assistant error message: {}", e);
                                }
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
        "sessions.rename" => handle_sessions_rename(req, conn, state).await,
        "sessions.set_pinned" => handle_sessions_set_pinned(req, conn, state).await,
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
        "models.fetch_remote" => handle_models_fetch_remote(req, state).await,
        "models.add" => handle_models_add(req, state).await,
        "models.remove" => handle_models_remove(req, state).await,
        "models.set_default" => handle_models_set_default(req, state).await,
        "mcp.list" => handle_mcp_list(req, state).await,
        "mcp.presets" => handle_mcp_presets(req, state).await,
        "mcp.add" => handle_mcp_add(req, state).await,
        "mcp.remove" => handle_mcp_remove(req, state).await,
        "mcp.connect" => handle_mcp_connect(req, state).await,
        "mcp.disconnect" => handle_mcp_disconnect(req, state).await,
        "mcp.auth_cancel" => handle_mcp_auth_cancel(req, state).await,
        "cron.list" => handle_cron_list(req, state).await,
        "tasks.schedule" => handle_tasks_schedule(req, state).await,
        "tasks.list" => handle_tasks_list(req, state).await,
        "tasks.delete" => handle_tasks_delete(req, state).await,
        "tasks.enable" => handle_tasks_enable(req, state).await,
        "tasks.disable" => handle_tasks_disable(req, state).await,
        "skills.list" => handle_skills_list(req, state).await,
        "skills.install" => handle_skills_install(req, state).await,
        "logs.subscribe" => handle_logs_subscribe(req, conn, state, cmd_tx).await,
        "logs.unsubscribe" => handle_logs_unsubscribe(req, conn, state).await,
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
        "permissions.request_macos_accessibility" => {
            handle_permissions_request_macos_accessibility(req).await
        }
        "subscribe" => handle_legacy_subscribe(req, conn, cmd_tx).await,
        "unsubscribe" => handle_legacy_unsubscribe(req, conn, cmd_tx).await,
        "subscribe_all" => {
            conn.write().await.subscriptions.clear();
            WsResponse::ok(&req.id, serde_json::json!({"status": "subscribed_all"}))
        }
        _ => error_method_not_found(&req.id, &req.method),
    }
}

async fn handle_permissions_request_macos_accessibility(req: &WsRequest) -> WsResponse {
    #[cfg(target_os = "macos")]
    {
        crate::computer::platform::macos::permissions::trigger_accessibility_prompt();
        crate::computer::platform::macos::permissions::open_accessibility_settings();
        WsResponse::ok(
            &req.id,
            serde_json::json!({
                "status": "prompt_triggered",
                "message": "System permission dialog triggered. Please allow access in System Settings → Privacy & Security → Accessibility, then restart Syscity."
            }),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        WsResponse::err(
            &req.id,
            "UNSUPPORTED_PLATFORM",
            "This permission request is only available on macOS",
        )
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
        if let Some(session) = state.auth.manager.validate_session(&token_str).await {
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
        if let Some(device_id) = state.auth.device_pairing_store.validate_token(token).await {
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
        .auth
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
            if let Err(e) = state.events.tx.send(GatewayEvent::DevicePairRequested {
                device_id: device.id.clone(),
                code: code.clone(),
                display_name: None,
            }) {
                warn!("Failed to broadcast DevicePairRequested for {}: {}", device.id, e);
            }
            error_invalid_request(
                &req.id,
                format!(
                    "Device pairing required. Use 'syscity device approve {}' to approve.",
                    code
                ),
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

/// Prompt for generating session titles via LLM.
const SESSION_TITLE_PROMPT: &str = "Summarize the following user message into a very short session title (at most 6 words, no punctuation, no explanation).\n\nMessage: {message}\n\nTitle:";

/// Generate a concise session title by asking an LLM to summarize the user's
/// first message.
async fn generate_session_title(
    router: &crate::model_router::ModelRouter,
    message: &str,
) -> crate::Result<String> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Ok("New Session".to_string());
    }

    let prompt = SESSION_TITLE_PROMPT.replace("{message}", trimmed);

    let messages = vec![
        ProviderMessage::system("You generate concise session titles."),
        ProviderMessage::user(prompt),
    ];

    let response = router.complete("default", messages, None).await?;
    let title = response
        .message
        .content
        .trim()
        .trim_matches(['"', '\'', '“', '”', '‘', '’'])
        .to_string();

    Ok(clean_session_title(&title))
}

/// Fallback title generation when LLM summarization fails.
fn fallback_session_name(message: &str) -> String {
    let name = message
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
    clean_session_title(&name)
}

/// Trim and truncate a session title to keep it sidebar-friendly.
fn clean_session_title(name: &str) -> String {
    let name = name.replace(['\n', '\r'], " ").trim().to_string();
    if name.len() > 40 {
        format!("{}...", &name[..40])
    } else if name.is_empty() {
        "New Session".to_string()
    } else {
        name
    }
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

    let (session_id, _is_new_session) = if let Some(sid) = params.session_id {
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
    if let Some(ref store) = state.agents.store {
        if let Err(e) = store
            .append_message(&AppendMessageParams {
                session_id: &session_id,
                role: "user",
                content: &params.message,
                ..Default::default()
            })
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
        let store = state.agents.store.clone();
        let router = state.infra.model_router.clone();
        let events = state.events.tx.clone();
        let sid = session_id.clone();
        let msg = params.message.clone();

        let name_task = tokio::spawn(async move {
            let name = generate_session_title(&router, &msg)
                .await
                .unwrap_or_else(|e| {
                    tracing::debug!("LLM session title generation failed: {}, using fallback", e);
                    fallback_session_name(&msg)
                });

            if let Some(ref s) = store {
                if let Err(e) = s.set_session_name(&sid, &name).await {
                    tracing::warn!("Failed to save session name for {}: {}", sid, e);
                } else {
                    tracing::info!("Session {} named: '{}'", sid, name);
                    if let Err(e) = events.send(GatewayEvent::SessionRenamed {
                        session_id: sid.clone(),
                        name: name.clone(),
                    }) {
                        tracing::debug!("No receivers for SessionRenamed event: {}", e);
                    }
                }
            }
        });
        state
            .task_registry
            .insert_join(format!("ws:session_name:{}", session_id), name_task)
            .await;
    }

    if let Some(ref store) = state.agents.store {
        if let Ok(Some(ps)) = store.load_session(&session_id).await {
            if let Some(ref bound_agent) = ps.metadata.bound_agent_id {
                let route = crate::inbound::RouteResult {
                    agent_id: bound_agent.clone(),
                    workspace_id: None,
                    persisted_binding: false,
                    is_fallback: false,
                };
                state.agents.router.bind_session(&session_id, &route).await;
            }
        }
    }

    if let Some(agent_id) = params.agent_id {
        let route = crate::inbound::RouteResult {
            agent_id,
            workspace_id: None,
            persisted_binding: false,
            is_fallback: false,
        };
        state.agents.router.bind_session(&session_id, &route).await;
    }

    // ── Smart name-based routing: "小王，xxx" -> route to secretary-xiaowang ──
    let mut final_message = params.message.clone();
    {
        let registry = state.agents.registry.read().await;
        // Try to extract a name prefix like "小王，" or "小王：" from the message.
        let trimmed = final_message.trim_start();
        if let Some((first_word, rest)) = trimmed.split_once(['，', ',', '：', ':', ' ', '\t']) {
            let name = first_word.trim();
            if !name.is_empty() {
                if let Some((personality, _matched_alias)) = registry.find_by_alias(name) {
                    let agent_id = personality.id.clone();
                    info!(
                        "Smart-routing session {} to agent '{}' (matched name: '{}' in message)",
                        session_id, agent_id, name
                    );
                    let route = crate::inbound::RouteResult {
                        agent_id: agent_id.clone(),
                        workspace_id: None,
                        persisted_binding: true,
                        is_fallback: false,
                    };
                    state.agents.router.bind_session(&session_id, &route).await;
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

    // Submit to the unified inbound entry channel instead of calling the
    // pipeline directly. The worker drives the message through the pipeline
    // and `process_routed_messages` dispatches to the agent.
    let routed = match state.pipelines.inbound_entry.send(incoming).await {
        Ok(()) => {
            // Best-effort synchronous resolution so the WebSocket client gets
            // immediate feedback. The resolved agent is also what will receive
            // the message via `routed_tx`.
            Some(state.agents.router.resolve_by_session(&session_id).await)
        }
        Err(e) => {
            return WsResponse::err(
                &req.id,
                "enqueue_failed",
                format!("Failed to enqueue message: {}", e),
            );
        }
    };

    let is_new_session = {
        let mut cg = conn.write().await;
        let is_new = !cg.subscriptions.contains(&session_id);
        if is_new {
            cg.subscriptions.push(session_id.clone());
        }
        is_new
    };

    if is_new_session {
        if let Err(e) = state
            .events
            .tx
            .send(crate::gateway::GatewayEvent::SessionCreated {
                session_id: session_id.clone(),
                agent_id: routed
                    .as_ref()
                    .map(|r| r.agent_id.clone())
                    .unwrap_or_default(),
                user_id: user_id.clone(),
            })
        {
            warn!("Failed to broadcast SessionCreated for {}: {}", session_id, e);
        }
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "accepted",
            "session_id": session_id,
            "agent_id": routed.map(|r| r.agent_id).unwrap_or_default(),
        }),
    )
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
        /// Optional timestamp in milliseconds. If provided, only messages with
        /// `created_at` strictly less than this value are returned (older
        /// messages).
        #[serde(default)]
        before: Option<i64>,
    }

    fn default_limit() -> usize {
        100
    }

    let params: HistoryParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let before = params.before.and_then(DateTime::from_timestamp_millis);

    let messages = if let Some(ref store) = state.agents.store {
        match store
            .get_messages(&params.session_id, params.limit as i64, before)
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
                            "timestamp": dt.timestamp_millis(),
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
            "has_more": messages.len() == params.limit,
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

    if let Err(e) = state.agents.acp.cancel(params.session_id.clone()).await {
        warn!("Failed to cancel session {} during abort: {}", params.session_id, e);
    }
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "aborted",
            "session_id": params.session_id,
        }),
    )
}

async fn handle_sessions_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let sessions: Vec<serde_json::Value> = if let Some(ref store) = state.agents.store {
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
                        "pinned": meta.pinned,
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
        let mgr = state.agents.manager.read().await;
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
        #[serde(default)]
        agent_id: Option<String>,
    }

    let params: CreateParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let session_id = params
        .session_id
        .unwrap_or_else(|| format!("{}:{}", channel, user));

    {
        let mut mgr = state.agents.manager.write().await;
        mgr.create_session(session_id.clone());
    }

    let mut metadata = crate::agent::session_store::SessionMetadata::new(
        &session_id,
        params.agent_id.as_deref().unwrap_or(""),
        &channel,
        &user,
    );
    metadata.bound_agent_id = params.agent_id.clone();

    if let Some(ref store) = state.agents.store {
        if let Err(e) = store.save_session(&session_id, &metadata, "{}").await {
            warn!("Failed to save session {}: {}", session_id, e);
        }
    }

    // Bind the session to the requested agent so future messages route there.
    if let Some(agent_id) = params.agent_id {
        if !agent_id.is_empty() {
            let route = crate::inbound::RouteResult {
                agent_id,
                workspace_id: None,
                persisted_binding: true,
                is_fallback: false,
            };
            state.agents.router.bind_session(&session_id, &route).await;
        }
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
        let mut mgr = state.agents.manager.write().await;
        mgr.terminate_session(&params.session_id).await;
    }

    if let Some(ref store) = state.agents.store {
        if let Err(e) = store.delete_session(&params.session_id).await {
            warn!("Failed to delete session {}: {}", params.session_id, e);
        }
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({ "status": "deleted", "session_id": params.session_id }),
    )
}

async fn handle_sessions_rename(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct RenameParams {
        session_id: String,
        name: String,
    }

    let params: RenameParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let trimmed = params.name.trim();
    if trimmed.is_empty() {
        return WsResponse::err(&req.id, "INVALID_REQUEST", "session name cannot be empty");
    }

    if let Some(ref store) = state.agents.store {
        if let Err(e) = store.set_session_name(&params.session_id, trimmed).await {
            warn!("Failed to rename session {}: {}", params.session_id, e);
            return WsResponse::err(&req.id, "INTERNAL_ERROR", e.to_string());
        }
    }

    // Broadcast the rename event so all connected clients update immediately.
    if let Err(e) = state.events.tx.send(GatewayEvent::SessionRenamed {
        session_id: params.session_id.clone(),
        name: trimmed.to_string(),
    }) {
        tracing::debug!("No receivers for SessionRenamed event: {}", e);
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "renamed",
            "session_id": params.session_id,
            "name": trimmed,
        }),
    )
}

async fn handle_sessions_set_pinned(
    req: &WsRequest,
    _conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct SetPinnedParams {
        session_id: String,
        pinned: bool,
    }

    let params: SetPinnedParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    if let Some(ref store) = state.agents.store {
        if let Err(e) = store
            .set_session_pinned(&params.session_id, params.pinned)
            .await
        {
            warn!("Failed to set pinned status for session {}: {}", params.session_id, e);
            return WsResponse::err(&req.id, "INTERNAL_ERROR", e.to_string());
        }
    }

    if let Err(e) = state.events.tx.send(GatewayEvent::SessionPinned {
        session_id: params.session_id.clone(),
        pinned: params.pinned,
    }) {
        tracing::debug!("No receivers for SessionPinned event: {}", e);
    }

    WsResponse::ok(&req.id, serde_json::json!({ "status": "ok" }))
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

    if let Err(e) = state.agents.acp.cancel(params.session_id.clone()).await {
        warn!("Failed to cancel session {} during reset: {}", params.session_id, e);
    }

    if let Some(ref store) = state.agents.store {
        if let Err(e) = store.delete_session(&params.session_id).await {
            warn!("Failed to delete session {} during reset: {}", params.session_id, e);
        }
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

    if let Err(e) = cmd_tx
        .send(WsCommand::Subscribe(params.session_ids.clone()))
        .await
    {
        warn!("Failed to send subscribe command: {}", e);
    }

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

    if let Err(e) = cmd_tx
        .send(WsCommand::Unsubscribe(params.session_ids.clone()))
        .await
    {
        warn!("Failed to send unsubscribe command: {}", e);
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "unsubscribed": params.session_ids,
        }),
    )
}

async fn handle_agents_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agents = {
        let agents = state.agents.agents.read().await;
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
        let agents = state.agents.agents.read().await;
        agents.get(&params.agent_id).cloned()
    };

    let personality = {
        let registry = state.agents.registry.read().await;
        registry.get(&params.agent_id).cloned()
    };

    match agent {
        Some(handle) => {
            let cfg = &handle.config;
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "agent_id": params.agent_id,
                    "busy": handle.busy.load(std::sync::atomic::Ordering::Acquire),
                    "status": if handle.busy.load(std::sync::atomic::Ordering::Acquire) { "busy" } else { "idle" },
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
                let cfg = p.to_agent_config();
                let config_json = match serde_json::to_value(&cfg) {
                    Ok(v) => v,
                    Err(e) => {
                        return WsResponse::err(
                            &req.id,
                            "SERIALIZE_FAILED",
                            format!("Failed to serialize agent config: {}", e),
                        );
                    }
                };
                WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "agent_id": params.agent_id,
                        "busy": false,
                        "status": "stopped",
                        "config": config_json,
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
    let registry = state.agents.registry.read().await;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entries: Vec<serde_json::Value> = Vec::new();

    // 1. Registry-discovered agents from disk
    for id in registry.list() {
        if let Some(p) = registry.get(&id) {
            seen.insert(id.clone());
            entries.push(serde_json::json!({
                "id": p.id,
                "display_name": p.display_name(),
                "emoji": p.emoji(),
                "is_valid": p.is_valid,
                "has_heartbeat": !p.heartbeat.is_empty(),
            }));
        }
    }

    // 2. Runtime-spawned agents not in registry (e.g. default)
    {
        let agents = state.agents.agents.read().await;
        for id in agents.keys() {
            if !seen.contains(id) {
                entries.push(serde_json::json!({
                    "id": id,
                    "display_name": id.as_str(),
                    "emoji": "🤖",
                    "is_valid": true,
                    "has_heartbeat": false,
                }));
            }
        }
    }

    WsResponse::ok(&req.id, serde_json::json!({ "agents": entries, "count": entries.len() }))
}

async fn handle_health(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agent_count = {
        let agents = state.agents.agents.read().await;
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
    let subagents = state.agents.acp.list_subagents().await;
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
        .auth
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
        mode,
        thread_binding: ThreadBinding::Auto,
        system_prompt: None,
        timeout_seconds: Some(300),
    };

    match state
        .agents
        .acp
        .spawn_subagent(session_id.clone(), parent_id.clone(), config)
        .await
    {
        Ok(handle) => {
            let subagent_id = handle.id.clone();

            state
                .auth
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

            match state.agents.acp.send_message(&subagent_id, message).await {
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
                    if let Err(shutdown_err) =
                        state.agents.acp.shutdown_subagent(&subagent_id).await
                    {
                        warn!(
                            "Failed to shutdown subagent {} after task failure: {}",
                            subagent_id, shutdown_err
                        );
                    }
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
                .auth
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
    match state.agents.acp.terminate_session(&session_id).await {
        Ok(count) => {
            state
                .auth
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
                .auth
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
    let subagents = state.agents.acp.list_session_subagents(&session_id).await;

    if subagents.is_empty() {
        return WsResponse::err(&req.id, "NO_ACTIVE_SUBAGENTS", "No active subagents in session");
    }

    let subagent = &subagents[0];
    let message =
        IncomingMessage::new("ws-user".to_string(), session_id.to_string(), params.message);

    match state.agents.acp.send_message(&subagent.id, message).await {
        Ok(response) => {
            state
                .auth
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
                .auth
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

    match state.agents.acp.get_status(params.session_id.clone()).await {
        Ok(Some(status)) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "session_id": status.session_id,
                "runtime_state": format!("{}", status.runtime_state),
                "mode": format!("{:?}", status.mode),
                "current_iteration": status.current_iteration,
                "max_iterations": status.max_iterations,
            }),
        ),
        Ok(None) => WsResponse::err(&req.id, "SESSION_NOT_FOUND", "Session not found"),
        Err(e) => WsResponse::err(&req.id, "ACP_ERROR", format!("Failed to get status: {}", e)),
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

    match state.agents.acp.pause(params.session_id.clone()).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "session_id": params.session_id,
                "action": "pause",
                "status": "requested",
            }),
        ),
        Err(e) => WsResponse::err(
            &req.id,
            "PAUSE_FAILED",
            format!("Failed to pause session {}: {}", params.session_id, e),
        ),
    }
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

    match state.agents.acp.resume(params.session_id.clone()).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "session_id": params.session_id,
                "action": "resume",
                "status": "requested",
            }),
        ),
        Err(e) => WsResponse::err(
            &req.id,
            "RESUME_FAILED",
            format!("Failed to resume session {}: {}", params.session_id, e),
        ),
    }
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

    match state.agents.acp.step(params.session_id.clone()).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "session_id": params.session_id,
                "action": "step",
                "status": "requested",
            }),
        ),
        Err(e) => WsResponse::err(
            &req.id,
            "STEP_FAILED",
            format!("Failed to step session {}: {}", params.session_id, e),
        ),
    }
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

    match state.agents.acp.cancel(params.session_id.clone()).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "session_id": params.session_id,
                "action": "cancel",
                "status": "requested",
            }),
        ),
        Err(e) => WsResponse::err(
            &req.id,
            "CANCEL_FAILED",
            format!("Failed to cancel session {}: {}", params.session_id, e),
        ),
    }
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
    let tree = state.agents.acp.get_subagent_tree(&session_id).await;

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
    let agents = state.agents.agents.read().await;
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

    // Attach structured tracing context for this request.
    // The _guard is explicitly dropped before .await because Entered is !Send.
    let ctx = RequestContext::new(Some(session_id.clone()), Some(params.user_id.clone()));
    let span = ctx.attach_to_span();
    let _guard = span.clone().entered();

    let incoming = crate::channels::IncomingMessage::new(
        params.user_id.clone(),
        session_id.clone(),
        params.message,
    );
    drop(_guard);

    match state
        .agents
        .acp
        .execute_session_with_max_iterations(agent_handle.agent, incoming, params.max_iterations)
        .instrument(span)
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
    let agents = state.agents.agents.read().await;
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

    // Attach structured tracing context for this request.
    // The _guard is explicitly dropped before .await because Entered is !Send.
    let ctx = RequestContext::new(Some(session_id.clone()), Some(params.user_id.clone()));
    let span = ctx.attach_to_span();
    let _guard = span.clone().entered();

    let incoming = crate::channels::IncomingMessage::new(
        params.user_id.clone(),
        session_id.clone(),
        params.message,
    );
    drop(_guard);

    match state
        .agents
        .acp
        .execute_run_with_max_iterations(agent_handle.agent, incoming, params.max_iterations)
        .instrument(span)
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

    if let Err(e) = cmd_tx.send(WsCommand::Subscribe(params.session_ids)).await {
        warn!("Failed to send legacy subscribe command: {}", e);
    }

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

    if let Err(e) = cmd_tx
        .send(WsCommand::Unsubscribe(params.session_ids))
        .await
    {
        warn!("Failed to send legacy unsubscribe command: {}", e);
    }

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
            "search": {
                "provider": config.search.provider,
                "providers": config.search.providers,
                "has_api_key": !config.search.api_key.is_empty(),
                "keys": {
                    "tavily": (!config.search.keys.get("tavily").is_none_or(|k| k.is_empty())).to_string(),
                    "serpapi": (!config.search.keys.get("serpapi").is_none_or(|k| k.is_empty())).to_string(),
                    "exa": (!config.search.keys.get("exa").is_none_or(|k| k.is_empty())).to_string(),
                    "firecrawl": (!config.search.keys.get("firecrawl").is_none_or(|k| k.is_empty())).to_string(),
                    "bing": (!config.search.keys.get("bing").is_none_or(|k| k.is_empty())).to_string(),
                    "google": (!config.search.keys.get("google").is_none_or(|k| k.is_empty())).to_string(),
                    "google_cx": (!config.search.keys.get("google_cx").is_none_or(|k| k.is_empty())).to_string(),
                    "brave": (!config.search.keys.get("brave").is_none_or(|k| k.is_empty())).to_string(),
                },
            },
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

    // Handle model switching outside the config write lock so the lock is not
    // held across an async model-router operation.
    let model_update = if params.path == "model" {
        if let Some(v) = params.value.as_str() {
            match state.infra.model_router.switch_default_model(v).await {
                Ok(()) => Some(v.to_string()),
                Err(e) => {
                    return WsResponse::err(
                        &req.id,
                        "CONFIG_ERROR",
                        format!("Failed to switch model: {}", e),
                    );
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let mut config_guard = state.config.write().await;
    let config = Arc::make_mut(&mut config_guard);

    match params.path.as_str() {
        "model" => {
            if let Some(v) = model_update {
                config.model = v;
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
                other => {
                    return WsResponse::err(
                        &req.id,
                        "INVALID_CHANNEL_TYPE",
                        format!("Unknown channel type: {}", other),
                    )
                }
            };
            let mut ch = crate::gateway::ChannelConfig::new(channel_type);
            if let Some(v) = payload.enabled {
                ch.enabled = v;
            }
            if let Some(v) = payload.agent_id {
                ch.agent_id = Some(v);
            }
            if let Some(v) = payload.credentials {
                ch.credentials = v;
            }
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
                    if let Some(v) = payload.enabled {
                        ch.enabled = v;
                    }
                    if let Some(v) = payload.agent_id {
                        ch.agent_id = Some(v);
                    }
                    if let Some(v) = payload.credentials {
                        ch.credentials = v;
                    }
                }
                None => {
                    return WsResponse::err(
                        &req.id,
                        "CHANNEL_NOT_FOUND",
                        format!("Channel '{}' not found", payload.name),
                    )
                }
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
                None => {
                    return WsResponse::err(
                        &req.id,
                        "CHANNEL_NOT_FOUND",
                        format!("Channel '{}' not found", payload.name),
                    )
                }
            }
        }
        "search.provider" => {
            if let Some(v) = params.value.as_str() {
                config.search.provider = v.to_string();
            }
        }
        "search.providers" => {
            if let Some(arr) = params.value.as_array() {
                config.search.providers = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }
        }
        _ if params.path.starts_with("search.keys.") => {
            let key_name = params.path.strip_prefix("search.keys.").unwrap_or("");
            if !key_name.is_empty() {
                match &params.value {
                    serde_json::Value::String(v) if !v.is_empty() => {
                        config.search.keys.insert(key_name.to_string(), v.clone());
                    }
                    _ => {
                        config.search.keys.remove(key_name);
                    }
                }
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

    // Persist config to disk so changes survive restarts and trigger hot-reload.
    // Keep the write lock held across persistence so concurrent writers cannot
    // overwrite our update before it is serialized.
    if let Some(config_path) = state.config_path.clone() {
        if let Err(e) = persist_config_atomic(&config_guard, &config_path).await {
            return WsResponse::err(
                &req.id,
                "PERSIST_FAILED",
                format!("Config updated in memory but failed to persist: {}", e),
            );
        }
    }
    drop(config_guard);

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
    let aliases = state.infra.model_router.aliases_with_configs().await;
    let entries: Vec<serde_json::Value> = aliases
        .iter()
        .map(|(name, alias)| {
            serde_json::json!({
                "id": name,
                "name": format!("{} ({})", name, alias.model),
                "provider": alias.provider,
            })
        })
        .collect();
    let default_model = state.infra.model_router.get_default_model().await;
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
    let builtins = crate::providers::preset::builtin_providers();
    let list: Vec<serde_json::Value> = presets
        .into_iter()
        .map(|(name, p)| {
            // Enrich with protocol/auth info from the TOML registry when the
            // preset exists there (custom does not).
            let builtin = builtins.get(name.as_str());
            let protocol = builtin.and_then(|b| b.variants.first()).map(|v| v.protocol);
            let needs_api_key = builtin
                .and_then(|b| b.variants.first())
                .map(|v| v.auth_method != crate::providers::AuthMethod::None)
                .unwrap_or(true);
            // Fall back to the TOML registry base URL when the legacy preset
            // does not define one (e.g. Anthropic, Gemini).
            let base_url = p.default_base_url.or_else(|| {
                builtin
                    .and_then(|b| b.variants.first())
                    .map(|v| v.default_base_url.clone())
            });
            serde_json::json!({
                "name": name,
                "display_name": p.display_name,
                "base_url": base_url,
                "models": p.models,
                "protocol": protocol,
                "needs_api_key": needs_api_key,
            })
        })
        .collect();
    WsResponse::ok(&req.id, serde_json::json!({ "presets": list }))
}

/// Build the list-models endpoint URL for a protocol.
fn models_endpoint_url(protocol: crate::providers::Protocol, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match protocol {
        crate::providers::Protocol::OpenAi => format!("{base}/models"),
        crate::providers::Protocol::Anthropic => format!("{base}/v1/models"),
        crate::providers::Protocol::Gemini => format!("{base}/models"),
    }
}

/// Parse model IDs from an OpenAI/Anthropic-style `{ "data": [{ "id": ... }] }` body.
fn parse_data_models(body: &serde_json::Value) -> Vec<String> {
    body.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse model IDs from a Gemini `{ "models": [{ "name": "models/..." }] }` body.
fn parse_gemini_models(body: &serde_json::Value) -> Vec<String> {
    body.get("models")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                .map(|n| n.strip_prefix("models/").unwrap_or(n).to_string())
                .collect()
        })
        .unwrap_or_default()
}

async fn handle_models_fetch_remote(req: &WsRequest, _state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct FetchRemotePayload {
        provider: String,
        base_url: Option<String>,
        api_key: Option<String>,
        /// Protocol override, required for providers not in the registry.
        protocol: Option<crate::providers::Protocol>,
    }
    let payload: FetchRemotePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    // Resolve protocol / default base URL / auth method from the TOML registry.
    let builtins = crate::providers::preset::builtin_providers();
    let variant = builtins
        .get(payload.provider.as_str())
        .and_then(|b| b.variants.first());

    let protocol = match payload.protocol.or_else(|| variant.map(|v| v.protocol)) {
        Some(p) => p,
        None => {
            return WsResponse::err(
                &req.id,
                "PROTOCOL_REQUIRED",
                format!(
                    "Unknown provider '{}'; an explicit protocol is required",
                    payload.provider
                ),
            );
        }
    };

    let base_url = match payload
        .base_url
        .filter(|u| !u.is_empty())
        .or_else(|| variant.map(|v| v.default_base_url.clone()))
    {
        Some(u) => u,
        None => {
            return WsResponse::err(
                &req.id,
                "BASE_URL_REQUIRED",
                format!("Provider '{}' requires a base_url", payload.provider),
            );
        }
    };

    if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
        return WsResponse::err(
            &req.id,
            "INVALID_BASE_URL",
            "base_url must start with http:// or https://".to_string(),
        );
    }

    let auth_method = variant
        .map(|v| v.auth_method.clone())
        .unwrap_or(crate::providers::AuthMethod::Bearer);
    let api_key = payload.api_key.filter(|k| !k.is_empty());

    let static_fallback = || {
        crate::model_router::provider_presets()
            .get(&payload.provider)
            .map(|p| p.models.clone())
            .unwrap_or_default()
    };

    let url = match variant.and_then(|v| v.models_endpoint.as_deref()) {
        Some(endpoint) => format!("{}{}", base_url.trim_end_matches('/'), endpoint),
        None => models_endpoint_url(protocol, &base_url),
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "models": static_fallback(),
                    "source": "static",
                    "error": format!("HTTP client error: {e}"),
                }),
            );
        }
    };

    let mut request = client.get(&url);
    match (&auth_method, &api_key) {
        (crate::providers::AuthMethod::Bearer, Some(key)) => {
            request = request.bearer_auth(key);
        }
        (crate::providers::AuthMethod::ApiKeyHeader, Some(key)) => {
            request = request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01");
        }
        (crate::providers::AuthMethod::GoogleApiKey, Some(key)) => {
            request = request.header("x-goog-api-key", key);
        }
        (crate::providers::AuthMethod::CustomHeader { name }, Some(key)) => {
            request = request.header(name, key);
        }
        _ => {}
    }

    match request.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(body) => {
                let models = match protocol {
                    crate::providers::Protocol::Gemini => parse_gemini_models(&body),
                    _ => parse_data_models(&body),
                };
                if models.is_empty() {
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "models": static_fallback(),
                            "source": "static",
                            "error": "Provider returned an empty model list",
                        }),
                    )
                } else {
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({ "models": models, "source": "remote" }),
                    )
                }
            }
            Err(e) => WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "models": static_fallback(),
                    "source": "static",
                    "error": format!("Failed to parse provider response: {e}"),
                }),
            ),
        },
        Ok(resp) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "models": static_fallback(),
                "source": "static",
                "error": format!("Provider returned HTTP {}", resp.status()),
            }),
        ),
        Err(e) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "models": static_fallback(),
                "source": "static",
                "error": format!("Failed to reach provider: {e}"),
            }),
        ),
    }
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
                payload
                    .base_url
                    .clone()
                    .or_else(|| p.default_base_url.clone()),
            ),
            None => (
                crate::model_router::ProviderType::Custom { name: provider_name.clone() },
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
            let mut config_guard = state.config.write().await;
            Arc::make_mut(&mut config_guard)
                .providers
                .insert(provider_name.clone(), provider_config.clone());
        }

        // Register with model router
        if let Err(e) = state
            .infra
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
    state.infra.model_router.set_alias(alias).await;

    // If this is the first alias, auto-set it as default
    let aliases = state.infra.model_router.list_aliases().await;
    if aliases.len() == 1 {
        if let Err(e) = state
            .infra
            .model_router
            .switch_default_model(&payload.name)
            .await
        {
            warn!("Failed to switch default model to {}: {}", payload.name, e);
        }
    }

    // Register in catalog for discovery
    let entry = crate::model_router::ModelCatalogEntry::new(
        payload.name.clone(),
        format!("{} ({})", payload.name, payload.name),
        payload.name.clone(),
    )
    .with_alias(payload.name.clone());
    state.infra.model_router.model_catalog.register(entry).await;

    // Persist GatewayConfig to config.toml
    if let Some(config_path) = state.config_path.clone() {
        let config_guard = state.config.read().await;
        if let Err(e) = persist_config_atomic(&config_guard, &config_path).await {
            return WsResponse::err(
                &req.id,
                "PERSIST_FAILED",
                format!("Model added but failed to persist config: {}", e),
            );
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
    let removed = state.infra.model_router.remove_alias(&payload.name).await;
    if removed {
        WsResponse::ok(&req.id, serde_json::json!({ "status": "removed" }))
    } else {
        WsResponse::err(
            &req.id,
            "MODEL_NOT_FOUND",
            format!("Model alias '{}' not found", payload.name),
        )
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
    match state
        .infra
        .model_router
        .switch_default_model(&payload.name)
        .await
    {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "status": "ok", "default_model": payload.name }),
        ),
        Err(e) => WsResponse::err(&req.id, "MODEL_NOT_FOUND", format!("{}", e)),
    }
}

async fn handle_mcp_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let connected = state.tools.mcp_manager.list_servers().await;
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

/// A single entry in `~/.syscity/mcp.toml`.
#[derive(Debug, Deserialize)]
struct McpPresetEntry {
    display_name: String,
    description: String,
    logo_url: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    transport: String,
    /// Remote HTTP URL (SSE or streamable_http)
    url: Option<String>,
    /// OAuth / bearer auth configuration
    auth_type: Option<String>,
    client_id: Option<String>,
    auth_url: Option<String>,
    token_url: Option<String>,
    scopes: Option<String>,
}

/// Return MCP presets from `~/.syscity/mcp.toml`, each annotated with
/// whether the preset is currently enabled (present in config.toml).
async fn handle_mcp_presets(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let presets: Vec<serde_json::Value> = match &state.mcps_path {
        Some(path) if path.exists() => match tokio::fs::read_to_string(path).await {
            Ok(content) => match toml::from_str::<HashMap<String, McpPresetEntry>>(&content) {
                Ok(map) => {
                    let cfg = state.config.read().await;
                    map.into_iter()
                        .map(|(name, entry)| {
                            let enabled = cfg.mcp.servers.contains_key(&name);
                            serde_json::json!({
                                "name": name,
                                "display_name": entry.display_name,
                                "description": entry.description,
                                "logo_url": entry.logo_url,
                                "command": entry.command,
                                "args": entry.args,
                                "transport": entry.transport,
                                "url": entry.url,
                                "auth_type": entry.auth_type,
                                "client_id": entry.client_id,
                                "auth_url": entry.auth_url,
                                "token_url": entry.token_url,
                                "scopes": entry.scopes,
                                "enabled": enabled,
                            })
                        })
                        .collect()
                }
                Err(e) => {
                    warn!("Failed to parse mcp.toml: {}", e);
                    Vec::new()
                }
            },
            Err(e) => {
                warn!("Failed to read mcp.toml: {}", e);
                Vec::new()
            }
        },
        _ => Vec::new(),
    };
    WsResponse::ok(&req.id, serde_json::json!({ "presets": presets }))
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
        #[serde(default)]
        env: std::collections::HashMap<String, String>,
        auth_type: Option<String>,
        client_id: Option<String>,
        auth_url: Option<String>,
        token_url: Option<String>,
        scopes: Option<String>,
        #[serde(default)]
        working_dir: Option<String>,
        #[serde(default = "default_true")]
        auto_connect: bool,
    }
    fn default_true() -> bool {
        true
    }

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
        env: payload.env,
        working_dir: payload.working_dir.map(std::path::PathBuf::from),
        auto_connect: payload.auto_connect,
        auth_type: payload.auth_type,
        client_id: payload.client_id,
        auth_url: payload.auth_url,
        token_url: payload.token_url,
        scopes: payload.scopes,
        ..Default::default()
    };

    {
        let mut cfg_guard = state.config.write().await;
        Arc::make_mut(&mut cfg_guard)
            .mcp
            .servers
            .insert(payload.id.clone(), config.clone());
    }

    if payload.auto_connect {
        match state
            .tools
            .mcp_manager
            .connect(&payload.id, config.clone())
            .await
        {
            Ok(tools) => {
                // Register tools immediately so agents can use them without
                // a daemon restart.
                super::lifecycle::register_mcp_tools(state, &payload.id, &tools, config.max_tools)
                    .await;
            }
            Err(e) => {
                return WsResponse::err(
                    &req.id,
                    "MCP_CONNECT_FAILED",
                    format!("Saved config but failed to connect: {}", e),
                );
            }
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

    if let Err(e) = state.tools.mcp_manager.disconnect(&payload.id).await {
        warn!("Failed to disconnect MCP server {}: {}", payload.id, e);
    }
    let prefix = format!("mcp__{}__", payload.id);
    state.tools.registry.deregister_prefix(&prefix);

    {
        let mut cfg_guard = state.config.write().await;
        Arc::make_mut(&mut cfg_guard)
            .mcp
            .servers
            .remove(&payload.id);
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
            None => {
                return WsResponse::err(
                    &req.id,
                    "MCP_NOT_FOUND",
                    format!("MCP server '{}' not configured", payload.id),
                )
            }
        }
    };

    // If the server uses OAuth, check for stored tokens first
    if config.auth_type.as_deref() == Some("oauth2") {
        if !state.tools.mcp_manager.has_stored_token(&payload.id).await {
            // No valid stored token — start the OAuth flow
            match state
                .tools
                .mcp_manager
                .start_oauth_flow(&payload.id, &config)
                .await
            {
                Ok(auth_url) => {
                    return WsResponse::err(
                        &req.id,
                        "MCP_AUTH_REQUIRED",
                        serde_json::json!({
                            "auth_url": auth_url,
                            "server_id": payload.id,
                        })
                        .to_string(),
                    );
                }
                Err(e) => {
                    return WsResponse::err(
                        &req.id,
                        "MCP_AUTH_FAILED",
                        format!("Failed to start OAuth flow: {}", e),
                    );
                }
            }
        }

        // Load stored token and set on a fresh client before connecting
        let tokens = state.tools.mcp_manager.load_stored_token(&payload.id).await;
        if let Some(tokens) = tokens {
            let mut client = crate::tools::mcp::McpClient::new()
                .with_timeout(config.timeout_secs);
            client.set_access_token(tokens.access_token.clone());

            match client.connect(config.clone()).await {
                Ok(()) => {
                    let tools = client.get_tools().to_vec();
                    let client_arc = std::sync::Arc::new(tokio::sync::RwLock::new(client));
                    // Register through the manager using the pre-authenticated client
                    if let Err(e) = state
                        .tools
                        .mcp_manager
                        .register_client(&payload.id, client_arc, config.clone())
                        .await
                    {
                        return WsResponse::err(
                            &req.id,
                            "MCP_CONNECT_FAILED",
                            format!("{}", e),
                        );
                    }
                    super::lifecycle::register_mcp_tools(
                        state,
                        &payload.id,
                        &tools,
                        config.max_tools,
                    )
                    .await;
                    return WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "status": "connected",
                            "id": payload.id,
                            "tool_count": tools.len(),
                        }),
                    );
                }
                Err(e) => {
                    return WsResponse::err(
                        &req.id,
                        "MCP_CONNECT_FAILED",
                        format!("{}", e),
                    );
                }
            }
        }
    }

    match state
        .tools
        .mcp_manager
        .connect(&payload.id, config.clone())
        .await
    {
        Ok(tools) => {
            super::lifecycle::register_mcp_tools(state, &payload.id, &tools, config.max_tools)
                .await;
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "status": "connected",
                    "id": payload.id,
                    "tool_count": tools.len(),
                }),
            )
        }
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

    match state.tools.mcp_manager.disconnect(&payload.id).await {
        Ok(()) => {
            let prefix = format!("mcp__{}__", payload.id);
            state.tools.registry.deregister_prefix(&prefix);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "status": "disconnected", "id": payload.id }),
            )
        }
        Err(e) => WsResponse::err(&req.id, "MCP_DISCONNECT_FAILED", format!("{}", e)),
    }
}

async fn handle_mcp_auth_cancel(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct McpAuthCancelPayload {
        server_id: String,
    }
    let payload: McpAuthCancelPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    state
        .tools
        .mcp_manager
        .cancel_oauth(&payload.server_id)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "status": "cancelled", "server_id": payload.server_id }),
    )
}

async fn handle_cron_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let jobs = match state.scheduler.cron_scheduler.read().await.clone() {
        Some(s) => s.lock().await.list_jobs().await,
        None => Vec::new(),
    };
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "jobs": jobs,
            "count": jobs.len(),
        }),
    )
}

async fn handle_tasks_schedule(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct TaskSchedulePayload {
        id: String,
        name: String,
        #[serde(default)]
        description: String,
        schedule: ScheduleInput,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", content = "value")]
    enum ScheduleInput {
        #[serde(rename = "once")]
        Once(String),
        #[serde(rename = "interval")]
        Interval(u64),
        #[serde(rename = "cron")]
        Cron(String),
    }

    let payload: TaskSchedulePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let scheduler = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s,
        None => {
            return WsResponse::err(
                &req.id,
                "SCHEDULER_UNAVAILABLE",
                "Task scheduler is not running",
            )
        }
    };

    let schedule = match payload.schedule {
        ScheduleInput::Once(s) => crate::planner::Schedule::once(s),
        ScheduleInput::Interval(seconds) => crate::planner::Schedule::interval(seconds),
        ScheduleInput::Cron(expr) => crate::planner::Schedule::cron(expr),
    };

    let task =
        crate::planner::ScheduledTask::new(payload.id.clone(), payload.name, schedule, vec![])
            .with_description(payload.description);

    let scheduler = scheduler.lock().await;
    match scheduler.add(task).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "status": "scheduled",
                "id": payload.id,
            }),
        ),
        Err(e) => WsResponse::err(&req.id, "SCHEDULE_FAILED", format!("{}", e)),
    }
}

async fn handle_tasks_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let tasks = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s.lock().await.list().await,
        None => Vec::new(),
    };
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "tasks": tasks,
            "count": tasks.len(),
        }),
    )
}

async fn handle_tasks_delete(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct TaskDeletePayload {
        id: String,
    }
    let payload: TaskDeletePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let scheduler = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s,
        None => {
            return WsResponse::err(
                &req.id,
                "SCHEDULER_UNAVAILABLE",
                "Task scheduler is not running",
            )
        }
    };

    let scheduler = scheduler.lock().await;
    match scheduler.remove(&payload.id).await {
        Ok(true) => {
            WsResponse::ok(&req.id, serde_json::json!({ "status": "deleted", "id": payload.id }))
        }
        Ok(false) => {
            WsResponse::err(&req.id, "NOT_FOUND", format!("Task '{}' not found", payload.id))
        }
        Err(e) => WsResponse::err(&req.id, "DELETE_FAILED", format!("{}", e)),
    }
}

async fn handle_tasks_enable(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct TaskEnablePayload {
        id: String,
    }
    let payload: TaskEnablePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let scheduler = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s,
        None => {
            return WsResponse::err(
                &req.id,
                "SCHEDULER_UNAVAILABLE",
                "Task scheduler is not running",
            )
        }
    };

    let scheduler = scheduler.lock().await;
    match scheduler.enable(&payload.id).await {
        Ok(true) => {
            WsResponse::ok(&req.id, serde_json::json!({ "status": "enabled", "id": payload.id }))
        }
        Ok(false) => {
            WsResponse::err(&req.id, "NOT_FOUND", format!("Task '{}' not found", payload.id))
        }
        Err(e) => WsResponse::err(&req.id, "ENABLE_FAILED", format!("{}", e)),
    }
}

async fn handle_tasks_disable(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct TaskDisablePayload {
        id: String,
    }
    let payload: TaskDisablePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let scheduler = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s,
        None => {
            return WsResponse::err(
                &req.id,
                "SCHEDULER_UNAVAILABLE",
                "Task scheduler is not running",
            )
        }
    };

    let scheduler = scheduler.lock().await;
    match scheduler.disable(&payload.id).await {
        Ok(true) => {
            WsResponse::ok(&req.id, serde_json::json!({ "status": "disabled", "id": payload.id }))
        }
        Ok(false) => {
            WsResponse::err(&req.id, "NOT_FOUND", format!("Task '{}' not found", payload.id))
        }
        Err(e) => WsResponse::err(&req.id, "DISABLE_FAILED", format!("{}", e)),
    }
}

async fn handle_skills_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let skills = {
        let sm = state.tools.skills_manager.read().await;
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
        return WsResponse::err(
            &req.id,
            "INTERNAL_ERROR",
            format!("Failed to create skill directory: {}", e),
        );
    }

    if let Some(zip_base64) = payload.zip_base64 {
        // Decode base64 ZIP
        let zip_bytes = match base64::engine::general_purpose::STANDARD.decode(&zip_base64) {
            Ok(b) => b,
            Err(e) => {
                return WsResponse::err(
                    &req.id,
                    "INVALID_CONTENT",
                    format!("Invalid base64: {}", e),
                )
            }
        };

        // Guard against oversized ZIPs before blocking the thread pool.
        const MAX_ZIP_BYTES: usize = 64 * 1024 * 1024;
        const MAX_ZIP_ENTRIES: usize = 10_000;
        const MAX_ENTRY_BYTES: usize = 16 * 1024 * 1024;
        if zip_bytes.len() > MAX_ZIP_BYTES {
            return WsResponse::err(
                &req.id,
                "INVALID_CONTENT",
                format!("ZIP exceeds maximum size of {} MB", MAX_ZIP_BYTES / (1024 * 1024)),
            );
        }

        let skill_dir_clone = skill_dir.clone();
        // Extract ZIP synchronously (ZipFile is not Send)
        #[allow(clippy::type_complexity)]
        let extract_task: tokio::task::JoinHandle<
            Result<Vec<(std::path::PathBuf, Vec<u8>)>, String>,
        > = tokio::task::spawn_blocking(move || {
            let cursor = std::io::Cursor::new(zip_bytes);
            let mut archive = match zip::ZipArchive::new(cursor) {
                Ok(a) => a,
                Err(e) => return Err(format!("Invalid ZIP: {}", e)),
            };

            if archive.len() > MAX_ZIP_ENTRIES {
                return Err(format!("ZIP contains too many entries (max {})", MAX_ZIP_ENTRIES));
            }

            let mut files: Vec<(std::path::PathBuf, Vec<u8>)> = Vec::new();
            let mut total_uncompressed: usize = 0;
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
                    let size = file.size() as usize;
                    if size > MAX_ENTRY_BYTES {
                        return Err(format!(
                            "ZIP entry '{}' exceeds maximum size of {} MB",
                            outpath.display(),
                            MAX_ENTRY_BYTES / (1024 * 1024)
                        ));
                    }
                    total_uncompressed = total_uncompressed.saturating_add(size);
                    if total_uncompressed > MAX_ZIP_BYTES {
                        return Err(format!(
                            "ZIP total uncompressed size exceeds {} MB",
                            MAX_ZIP_BYTES / (1024 * 1024)
                        ));
                    }
                    let mut contents = Vec::with_capacity(size);
                    if let Err(e) = std::io::Read::read_to_end(&mut file, &mut contents) {
                        return Err(format!("Failed to read ZIP entry: {}", e));
                    }
                    files.push((outpath, contents));
                }
            }
            Ok(files)
        });

        let files: Vec<(std::path::PathBuf, Vec<u8>)> =
            match tokio::time::timeout(std::time::Duration::from_secs(30), extract_task).await {
                Ok(Ok(Ok(f))) => f,
                Ok(Ok(Err(msg))) => return WsResponse::err(&req.id, "INVALID_CONTENT", msg),
                Ok(Err(_)) => {
                    return WsResponse::err(
                        &req.id,
                        "INTERNAL_ERROR",
                        "ZIP extraction task was cancelled".to_string(),
                    )
                }
                Err(_) => {
                    return WsResponse::err(
                        &req.id,
                        "INVALID_CONTENT",
                        "ZIP extraction timed out".to_string(),
                    )
                }
            };

        // Write extracted files
        for (outpath, contents) in files {
            if let Some(parent) = outpath.parent() {
                if !parent.exists() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        return WsResponse::err(
                            &req.id,
                            "INTERNAL_ERROR",
                            format!("Failed to create directory: {}", e),
                        );
                    }
                }
            }
            if let Err(e) = tokio::fs::write(&outpath, &contents).await {
                return WsResponse::err(
                    &req.id,
                    "INTERNAL_ERROR",
                    format!("Failed to write file: {}", e),
                );
            }
        }

        // Validate SKILL.md exists and is valid
        let skill_md_path = skill_dir.join("SKILL.md");
        if !skill_md_path.exists() {
            if let Err(e) = tokio::fs::remove_dir_all(&skill_dir).await {
                warn!("Failed to remove skill dir {}: {}", skill_dir.display(), e);
            }
            return WsResponse::err(
                &req.id,
                "INVALID_CONTENT",
                "ZIP must contain SKILL.md at the root",
            );
        }

        let skill_md_content = match tokio::fs::read_to_string(&skill_md_path).await {
            Ok(c) => c,
            Err(e) => {
                if let Err(rm_err) = tokio::fs::remove_dir_all(&skill_dir).await {
                    warn!("Failed to remove skill dir {}: {}", skill_dir.display(), rm_err);
                }
                return WsResponse::err(
                    &req.id,
                    "INVALID_CONTENT",
                    format!("Failed to read SKILL.md: {}", e),
                );
            }
        };

        if let Err(e) = crate::skills::parse_skill_md(&skill_md_content) {
            if let Err(rm_err) = tokio::fs::remove_dir_all(&skill_dir).await {
                warn!("Failed to remove skill dir {}: {}", skill_dir.display(), rm_err);
            }
            return WsResponse::err(&req.id, "INVALID_CONTENT", format!("Invalid SKILL.md: {}", e));
        }
    } else if let Some(content) = payload.content {
        // Legacy single-file install
        if let Err(e) = crate::skills::parse_skill_md(&content) {
            return WsResponse::err(
                &req.id,
                "INVALID_CONTENT",
                format!("Invalid skill markdown: {}", e),
            );
        }
        let skill_path = skill_dir.join("SKILL.md");
        if let Err(e) = tokio::fs::write(&skill_path, &content).await {
            return WsResponse::err(
                &req.id,
                "INTERNAL_ERROR",
                format!("Failed to write skill file: {}", e),
            );
        }
    } else {
        return WsResponse::err(
            &req.id,
            "INVALID_PARAMS",
            "Either content or zip_base64 is required",
        );
    }

    // Reload skills
    {
        let sm = state.tools.skills_manager.write().await;
        if let Err(e) = sm.load_all().await {
            return WsResponse::err(
                &req.id,
                "INTERNAL_ERROR",
                format!("Failed to reload skills: {}", e),
            );
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
    // Cancel any existing log subscription for this connection and remove its
    // task from the registry so we don't leak aborted tasks.
    let (conn_id, prev_cancel_tx) = {
        let cg = conn.write().await;
        let conn_id = cg.conn_id.clone();
        let prev_cancel_tx = cg.log_cancel_tx.clone();
        (conn_id, prev_cancel_tx)
    };
    if let Some(tx) = prev_cancel_tx {
        if let Err(e) = tx.send(()).await {
            warn!("Failed to cancel previous log tail for {}: {}", conn_id, e);
        }
    }
    state
        .task_registry
        .abort(&format!("ws:log_tail:{}", conn_id))
        .await;

    let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);
    {
        let mut cg = conn.write().await;
        cg.log_cancel_tx = Some(cancel_tx);
    }

    let log_tx = state.events.log_tx.clone();
    let cmd_tx = cmd_tx.clone();
    let task_registry = state.task_registry.clone();
    let shutdown_token = state.shutdown_token.clone();
    let conn_id_for_task = conn_id.clone();

    let task_handle = tokio::spawn(async move {
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
                        }))
                        .ok(),
                        seq: None,
                    };
                    if let Ok(text) = serde_json::to_string(&event) {
                        if cmd_tx.send(WsCommand::SendEvent(text)).await.is_err() {
                            warn!("Log tail send channel closed for {}", conn_id_for_task);
                            break;
                        }
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
                        if cmd_tx
                            .send(WsCommand::SendEvent(text))
                            .await
                            .is_err()
                        {
                            warn!("Log tail send channel closed for {}", conn_id_for_task);
                            break;
                        }
                    }
                }
                _ = cancel_rx.recv() => {
                    break;
                }
                _ = shutdown_token.cancelled() => {
                    info!("Log tail task received shutdown signal for {}", conn_id_for_task);
                    break;
                }
            }
        }
    });

    task_registry
        .insert_join(format!("ws:log_tail:{}", conn_id), task_handle)
        .await;

    WsResponse::ok(&req.id, serde_json::json!({ "status": "subscribed" }))
}

async fn handle_logs_unsubscribe(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let (conn_id, cancel_tx) = {
        let mut cg = conn.write().await;
        let conn_id = cg.conn_id.clone();
        let cancel_tx = cg.log_cancel_tx.take();
        (conn_id, cancel_tx)
    };
    if let Some(tx) = cancel_tx {
        if let Err(e) = tx.send(()).await {
            warn!("Failed to cancel log tail for {}: {}", conn_id, e);
        }
    }
    state
        .task_registry
        .abort(&format!("ws:log_tail:{}", conn_id))
        .await;

    WsResponse::ok(&req.id, serde_json::json!({ "status": "unsubscribed" }))
}

/// Persist GatewayConfig to config.toml.
async fn persist_config(state: &Arc<GatewayState>) -> Result<(), WsResponse> {
    if let Some(config_path) = state.config_path.clone() {
        let config_guard = state.config.read().await;
        match toml::to_string_pretty(&*config_guard) {
            Ok(toml_str) => {
                let tmp_path = config_path.with_extension("toml.tmp");
                if let Err(e) = tokio::fs::write(&tmp_path, toml_str).await {
                    return Err(WsResponse::err(
                        "persist",
                        "PERSIST_FAILED",
                        format!("Failed to write temporary config: {}", e),
                    ));
                }
                if let Err(e) = tokio::fs::rename(&tmp_path, &config_path).await {
                    return Err(WsResponse::err(
                        "persist",
                        "PERSIST_FAILED",
                        format!("Failed to atomically replace config: {}", e),
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

    fn make_req(id: &str, method: &str, params: serde_json::Value) -> WsRequest {
        WsRequest {
            frame_type: "req".to_string(),
            id: id.to_string(),
            method: method.to_string(),
            params: Some(params),
        }
    }

    #[test]
    fn test_models_endpoint_url() {
        use crate::providers::Protocol;
        assert_eq!(
            models_endpoint_url(Protocol::OpenAi, "https://api.openai.com/v1"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            models_endpoint_url(Protocol::Anthropic, "https://api.anthropic.com/"),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            models_endpoint_url(
                Protocol::Gemini,
                "https://generativelanguage.googleapis.com/v1beta"
            ),
            "https://generativelanguage.googleapis.com/v1beta/models"
        );
    }

    #[test]
    fn test_parse_data_models() {
        let body = serde_json::json!({
            "object": "list",
            "data": [
                { "id": "gpt-4o", "object": "model" },
                { "id": "gpt-4o-mini", "object": "model" },
                { "object": "model" }
            ]
        });
        assert_eq!(parse_data_models(&body), vec!["gpt-4o", "gpt-4o-mini"]);
        assert!(parse_data_models(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn test_parse_gemini_models() {
        let body = serde_json::json!({
            "models": [
                { "name": "models/gemini-2.0-flash" },
                { "name": "models/gemini-1.5-pro" }
            ]
        });
        assert_eq!(parse_gemini_models(&body), vec!["gemini-2.0-flash", "gemini-1.5-pro"]);
        assert!(parse_gemini_models(&serde_json::json!({})).is_empty());
    }

    #[tokio::test]
    async fn test_fetch_remote_unknown_provider_requires_protocol() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        let req = make_req(
            "r1",
            "models.fetch_remote",
            serde_json::json!({ "provider": "no-such-provider" }),
        );
        let res = handle_models_fetch_remote(&req, &state).await;
        assert!(!res.ok);
    }

    #[tokio::test]
    async fn test_fetch_remote_invalid_base_url() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        let req = make_req(
            "r1",
            "models.fetch_remote",
            serde_json::json!({ "provider": "openai", "base_url": "ftp://example.com" }),
        );
        let res = handle_models_fetch_remote(&req, &state).await;
        assert!(!res.ok);
    }

    #[tokio::test]
    async fn test_fetch_remote_unreachable_falls_back_to_static() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        let req = make_req(
            "r1",
            "models.fetch_remote",
            serde_json::json!({
                "provider": "openai",
                // Port 1 is reserved and unreachable, so the request fails fast.
                "base_url": "http://127.0.0.1:1"
            }),
        );
        let res = handle_models_fetch_remote(&req, &state).await;
        assert!(res.ok);
        let payload = res.payload.unwrap();
        assert_eq!(payload.get("source").unwrap(), "static");
        let models = payload.get("models").unwrap().as_array().unwrap();
        assert!(models.iter().any(|m| m == "gpt-4o"));
    }

    #[tokio::test]
    async fn test_handle_mcp_list_empty() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        let req = make_req("r1", "mcp.list", serde_json::json!({}));
        let res = handle_mcp_list(&req, &state).await;
        assert!(res.ok);
        let payload = res.payload.unwrap();
        let servers = payload.get("servers").unwrap().as_array().unwrap();
        assert!(servers.is_empty());
    }

    #[tokio::test]
    async fn test_handle_mcp_add_and_list() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        let req = make_req(
            "r1",
            "mcp.add",
            serde_json::json!({
                "id": "test-server",
                "transport": "stdio",
                "command": "echo",
                "args": ["hello"],
                "auto_connect": false,
            }),
        );
        let res = handle_mcp_add(&req, &state).await;
        assert!(res.ok, "add failed: {:?}", res.error);
        assert_eq!(res.payload.unwrap().get("status").unwrap(), "added");

        let req = make_req("r2", "mcp.list", serde_json::json!({}));
        let res = handle_mcp_list(&req, &state).await;
        assert!(res.ok);
        let servers = res
            .payload
            .unwrap()
            .get("servers")
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].get("id").unwrap(), "test-server");
        assert_eq!(servers[0].get("transport").unwrap(), "stdio");
        assert_eq!(servers[0].get("connected").unwrap(), false);
    }

    #[tokio::test]
    async fn test_handle_mcp_remove() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        // Add first
        let req = make_req(
            "r1",
            "mcp.add",
            serde_json::json!({
                "id": "to-remove",
                "transport": "stdio",
                "command": "echo",
                "args": [],
                "auto_connect": false,
            }),
        );
        let res = handle_mcp_add(&req, &state).await;
        assert!(res.ok);

        // Remove
        let req = make_req("r2", "mcp.remove", serde_json::json!({ "id": "to-remove" }));
        let res = handle_mcp_remove(&req, &state).await;
        assert!(res.ok);
        assert_eq!(res.payload.unwrap().get("status").unwrap(), "removed");

        // List should be empty
        let req = make_req("r3", "mcp.list", serde_json::json!({}));
        let res = handle_mcp_list(&req, &state).await;
        let payload = res.payload.unwrap();
        let servers = payload.get("servers").unwrap().as_array().unwrap();
        assert!(servers.is_empty());
    }

    #[tokio::test]
    async fn test_handle_mcp_connect_not_found() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        let req = make_req("r1", "mcp.connect", serde_json::json!({ "id": "missing" }));
        let res = handle_mcp_connect(&req, &state).await;
        assert!(!res.ok);
        assert_eq!(res.error.as_ref().unwrap().code, "MCP_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_handle_mcp_disconnect_not_connected() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        let req = make_req("r1", "mcp.disconnect", serde_json::json!({ "id": "nobody" }));
        let res = handle_mcp_disconnect(&req, &state).await;
        assert!(res.ok);
        assert_eq!(res.payload.unwrap().get("status").unwrap(), "disconnected");
    }

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
