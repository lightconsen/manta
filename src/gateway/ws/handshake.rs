//! Connection handshake: connect, hello/device auth, ping, session titles.

use super::*;
pub(super) async fn handle_connect(
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

pub(super) async fn handle_device_auth(
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

pub(super) fn handle_ping(req: &WsRequest) -> WsResponse {
    WsResponse::ok(&req.id, serde_json::json!({}))
}

/// Prompt for generating session titles via LLM.
const SESSION_TITLE_PROMPT: &str = "Summarize the following user message into a very short session title (at most 6 words, no punctuation, no explanation).\n\nMessage: {message}\n\nTitle:";

/// Generate a concise session title by asking an LLM to summarize the user's
/// first message.
pub(super) async fn generate_session_title(
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
pub(super) fn fallback_session_name(message: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::AuthMode;
    use crate::gateway::state_tests::{make_test_conn, make_test_state};
    use crate::gateway::ws::WsAuthResult;
    use crate::gateway::GatewayConfig;
    use crate::security::UserId;

    fn req(id: &str, params: Option<serde_json::Value>) -> WsRequest {
        WsRequest {
            frame_type: "req".into(),
            id: id.into(),
            method: "connect".into(),
            params,
        }
    }

    fn params(overrides: serde_json::Value) -> Option<serde_json::Value> {
        let mut p = serde_json::json!({ "protocol_version": PROTOCOL_VERSION });
        if let Some(obj) = overrides.as_object() {
            for (k, v) in obj {
                p[k] = v.clone();
            }
        }
        Some(p)
    }

    async fn state() -> Arc<GatewayState> {
        Arc::new(make_test_state(GatewayConfig::default()).await)
    }

    async fn dispatch_connect(
        conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
        state: &Arc<GatewayState>,
        auth_mode: AuthMode,
        params: Option<serde_json::Value>,
    ) -> WsResponse {
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<WsCommand>(1);
        let pre = WsAuthResult {
            user_id: UserId::new("u1"),
            scopes: vec!["chat".to_string()],
        };
        handle_connect(&req("r1", params), conn, state, &auth_mode, &cmd_tx, &pre).await
    }

    #[tokio::test]
    async fn connect_missing_params_errors() {
        let state = state().await;
        let conn = make_test_conn(&[]);
        let resp = dispatch_connect(&conn, &state, AuthMode::None, None).await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn connect_invalid_params_errors() {
        let state = state().await;
        let conn = make_test_conn(&[]);
        let resp =
            dispatch_connect(&conn, &state, AuthMode::None, Some(serde_json::json!({"nope": 1})))
                .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn connect_version_mismatch_errors() {
        let state = state().await;
        let conn = make_test_conn(&[]);
        let resp = dispatch_connect(
            &conn,
            &state,
            AuthMode::None,
            Some(serde_json::json!({ "protocol_version": 9999 })),
        )
        .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "VERSION_MISMATCH");
    }

    #[tokio::test]
    async fn connect_none_mode_hello_ok() {
        let state = state().await;
        let conn = make_test_conn(&[]);
        // Requesting "admin" grants it only if present in ALL_SCOPES.
        let resp = dispatch_connect(
            &conn,
            &state,
            AuthMode::None,
            params(serde_json::json!({
                "scopes": ["admin"],
                "client": { "id": "web", "version": "1.0" },
            })),
        )
        .await;
        assert!(resp.ok, "connect should succeed in None mode: {:?}", resp.error);
        let payload = resp.payload.as_ref().unwrap();
        assert_eq!(payload["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(payload["session_key"], "web:u1");
        assert!(payload["features"]
            .as_array()
            .unwrap()
            .contains(&"chat".into()));
        assert!(payload["scopes_granted"]
            .as_array()
            .unwrap()
            .contains(&"chat".into()));
        assert!(payload["scopes_granted"]
            .as_array()
            .unwrap()
            .contains(&"admin".into()));
        assert_eq!(payload["server"]["conn_id"], "test-conn");

        // Connection state mutated by the handshake.
        let cg = conn.read().await;
        assert!(cg.handshaked);
        assert!(cg.scopes.contains(&"admin".to_string()));
    }

    #[tokio::test]
    async fn connect_token_shared_secret_ok() {
        let mut config = GatewayConfig::default();
        config.security.auth_mode = AuthMode::Token;
        config.security.shared_token = Some("secret-token".to_string());
        let state = Arc::new(make_test_state(config).await);
        let conn = make_test_conn(&[]);
        let resp = dispatch_connect(
            &conn,
            &state,
            AuthMode::Token,
            params(serde_json::json!({
                "auth": { "token": "secret-token" },
                "scopes": ["read"],
            })),
        )
        .await;
        assert!(resp.ok, "shared token should authenticate: {:?}", resp.error);
        let payload = resp.payload.as_ref().unwrap();
        assert_eq!(payload["session_key"], "ws:shared");
        assert!(payload["scopes_granted"]
            .as_array()
            .unwrap()
            .contains(&"read".into()));
    }

    #[tokio::test]
    async fn connect_token_invalid_anonymous_ok() {
        let mut config = GatewayConfig::default();
        config.security.auth_mode = AuthMode::Token;
        config.security.shared_token = Some("secret-token".to_string());
        let state = Arc::new(make_test_state(config).await);
        let conn = make_test_conn(&[]);
        let resp =
            dispatch_connect(&conn, &state, AuthMode::Token, params(serde_json::json!({}))).await;
        assert!(resp.ok);
        let payload = resp.payload.as_ref().unwrap();
        assert_eq!(payload["session_key"], "ws:anonymous");
        assert!(payload["scopes_granted"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connect_tailscale_user_ok() {
        let state = state().await;
        let conn = make_test_conn(&[]);
        let resp =
            dispatch_connect(&conn, &state, AuthMode::Tailscale, params(serde_json::json!({})))
                .await;
        assert!(resp.ok);
        let payload = resp.payload.as_ref().unwrap();
        assert_eq!(payload["session_key"], "ws:tailscale");
        assert!(payload["scopes_granted"]
            .as_array()
            .unwrap()
            .contains(&"chat".into()));
    }

    #[tokio::test]
    async fn connect_device_without_identity_errors() {
        let mut config = GatewayConfig::default();
        config.security.auth_mode = AuthMode::Device;
        let state = Arc::new(make_test_state(config).await);
        let conn = make_test_conn(&[]);
        let resp =
            dispatch_connect(&conn, &state, AuthMode::Device, params(serde_json::json!({}))).await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_REQUEST");
    }

    #[test]
    fn ping_returns_empty_object() {
        let resp = handle_ping(&req("r1", None));
        assert!(resp.ok);
        assert!(resp.payload.as_ref().unwrap().is_object());
    }

    #[tokio::test]
    async fn generate_title_empty_message_early_returns() {
        let router = crate::model_router::ModelRouter::new(
            crate::model_router::ModelRouterConfig::default(),
        );
        let title = generate_session_title(&router, "   ").await.unwrap();
        assert_eq!(title, "New Session");
    }

    #[tokio::test]
    async fn fallback_name_keeps_first_six_words() {
        assert_eq!(
            fallback_session_name("one two three four five six seven"),
            "one two three four five six"
        );
    }

    #[tokio::test]
    async fn fallback_name_empty_returns_new_session() {
        assert_eq!(fallback_session_name("  "), "New Session");
    }

    #[tokio::test]
    async fn fallback_name_truncates_long_title() {
        let long = "a".repeat(60);
        let name = fallback_session_name(&long);
        assert!(name.ends_with("..."));
        assert!(name.len() <= 43);
    }

    #[tokio::test]
    async fn connect_sets_client_info() {
        let state = state().await;
        let conn = make_test_conn(&[]);
        let resp = dispatch_connect(
            &conn,
            &state,
            AuthMode::None,
            params(serde_json::json!({
                "client": { "id": "ios", "version": "2.1" },
            })),
        )
        .await;
        assert!(resp.ok);
        let cg = conn.read().await;
        let client = cg.client.as_ref().expect("client info stored");
        assert_eq!(client.id, "ios");
        assert_eq!(cg.scopes, vec!["chat".to_string()]);
    }
}
