//! QQ Bot WebSocket/Webhook Handler
//!
//! QQ Bot API uses WebSocket Gateway for real-time events, not HTTP webhooks.
//! This module provides:
//! 1. HTTP endpoint for QQ callback verification (if any)
//! 2. WebSocket Gateway client for receiving messages
//!
//! Docs: https://bot.q.qq.com/wiki/develop/api/

use super::WebhookState;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::{debug, error, warn};

type HmacSha256 = Hmac<Sha256>;

/// QQ webhook payload (for callbacks if supported)
#[derive(Debug, Deserialize, Clone)]
pub struct QqWebhook {
    /// Event type
    #[serde(rename = "t")]
    pub event_type: Option<String>,
    /// Event data
    #[serde(rename = "d")]
    pub data: Option<serde_json::Value>,
    /// Sequence number
    #[serde(rename = "s")]
    pub sequence: Option<u64>,
    /// OP code (WebSocket operation code)
    pub op: Option<u32>,
    /// ID
    pub id: Option<String>,
    /// Plain text (for some callback formats)
    #[serde(rename = "plain_token")]
    pub plain_token: Option<String>,
    /// Event timestamp
    #[serde(rename = "event_ts")]
    pub event_ts: Option<String>,
}

/// QQ callback response (for URL verification)
#[derive(Debug, Serialize)]
pub struct QqCallbackResponse {
    #[serde(rename = "plain_token")]
    pub plain_token: String,
    pub signature: String,
}

/// QQ message event data
#[derive(Debug, Deserialize, Clone)]
pub struct QqMessageEvent {
    pub id: String,
    #[serde(rename = "channel_id")]
    pub channel_id: Option<String>,
    #[serde(rename = "guild_id")]
    pub guild_id: Option<String>,
    pub content: String,
    pub timestamp: String,
    pub author: QqAuthor,
    #[serde(rename = "member")]
    pub member: Option<QqMember>,
    pub mentions: Option<Vec<QqUser>>,
    #[serde(rename = "message_reference")]
    pub message_reference: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QqAuthor {
    pub id: String,
    pub username: String,
    pub bot: Option<bool>,
    #[serde(rename = "avatar")]
    pub avatar: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QqMember {
    pub roles: Option<Vec<String>>,
    #[serde(rename = "joined_at")]
    pub joined_at: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QqUser {
    pub id: String,
    pub username: String,
    #[serde(rename = "avatar")]
    pub avatar: Option<String>,
}

/// QQ bot status/info
#[derive(Debug, Serialize)]
pub struct QqBotInfo {
    pub status: String,
    pub message: String,
    pub websocket_required: bool,
    pub note: String,
}

/// Create QQ webhook router
pub fn qq_webhook_router(state: WebhookState) -> Router {
    Router::new()
        .route("/", get(get_qq_info).post(handle_webhook))
        .route("/callback", post(handle_callback))
        .with_state(state)
}

/// Get QQ bot info (GET)
/// Returns information about QQ bot setup since it requires WebSocket
async fn get_qq_info(State(_state): State<WebhookState>) -> impl IntoResponse {
    let info = QqBotInfo {
        status: "info".to_string(),
        message: "QQ Bot requires WebSocket Gateway connection".to_string(),
        websocket_required: true,
        note: "Use 'manta channel start qq' to establish WebSocket connection. HTTP webhooks are not supported by QQ Bot API.".to_string(),
    };

    (StatusCode::OK, Json(info))
}

/// Handle incoming webhook events (POST)
/// Note: QQ primarily uses WebSocket, but this handles any HTTP callbacks
async fn handle_webhook(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    debug!("Received QQ webhook, headers: {:?}", headers);

    let config = match state.qq_config {
        Some(ref c) => c,
        None => {
            warn!("Received QQ webhook but QQ is not configured");
            return (StatusCode::SERVICE_UNAVAILABLE, "QQ not configured");
        }
    };

    // Verify signature if X-Signature header is present
    if let Some(sig_header) = headers.get("x-signature") {
        if let Ok(sig_str) = sig_header.to_str() {
            if !verify_qq_signature(&body, sig_str, &config.app_secret) {
                warn!("Invalid QQ webhook signature");
                return (StatusCode::FORBIDDEN, "Invalid signature");
            }
        }
    }

    // Parse the payload
    let payload: QqWebhook = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to parse QQ webhook: {}", e);
            return (StatusCode::BAD_REQUEST, "Invalid JSON");
        }
    };

    // Process the event
    if let Err(e) = process_event(&state, &payload, config).await {
        error!("Failed to process QQ event: {}", e);
    }

    (StatusCode::OK, "OK")
}

/// Handle QQ callback verification
/// QQ sends callbacks for certain events that require signature verification
async fn handle_callback(
    State(state): State<WebhookState>,
    Json(payload): Json<QqWebhook>,
) -> impl IntoResponse {
    debug!("Received QQ callback: {:?}", payload);

    let config = match state.qq_config {
        Some(ref c) => c,
        None => {
            warn!("Received QQ callback but QQ is not configured");
            return (StatusCode::SERVICE_UNAVAILABLE, "QQ not configured").into_response();
        }
    };

    // If this is a URL verification callback with plain_token
    if let Some(plain_token) = payload.plain_token {
        // Generate signature
        let signature = generate_callback_signature(&plain_token, &config.app_secret);
        let response = QqCallbackResponse { plain_token, signature };
        return (StatusCode::OK, Json(response)).into_response();
    }

    // Process other callback events
    if let Err(e) = process_event(&state, &payload, config).await {
        error!("Failed to process QQ callback: {}", e);
    }

    (StatusCode::OK, "OK").into_response()
}

/// Verify QQ webhook signature
/// Uses HMAC-SHA256 of the request body
fn verify_qq_signature(body: &[u8], signature: &str, secret: &str) -> bool {
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    mac.update(body);
    let result = mac.finalize();
    let computed_sig = hex::encode(result.into_bytes());

    // Signature might be hex or base64 encoded
    use base64::{engine::general_purpose, Engine as _};
    computed_sig == signature || general_purpose::STANDARD.encode(&computed_sig) == signature
}

/// Generate callback signature for QQ URL verification
fn generate_callback_signature(plain_token: &str, secret: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(plain_token.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// Process QQ event
async fn process_event(
    state: &WebhookState,
    payload: &QqWebhook,
    config: &crate::channels::QqConfig,
) -> crate::Result<()> {
    // Check event type
    let event_type = payload.event_type.as_deref().unwrap_or("");

    match event_type {
        "MESSAGE_CREATE" | "AT_MESSAGE_CREATE" | "DIRECT_MESSAGE_CREATE" => {
            if let Some(ref data) = payload.data {
                if let Ok(msg_event) = serde_json::from_value::<QqMessageEvent>(data.clone()) {
                    if let Err(e) = process_message(state, &msg_event, config).await {
                        error!("Failed to process QQ message: {}", e);
                    }
                }
            }
        }
        "GUILD_MEMBER_ADD" | "GUILD_MEMBER_UPDATE" | "GUILD_MEMBER_REMOVE" => {
            debug!("Ignoring guild member event");
        }
        _ => {
            debug!("Unhandled QQ event type: {}", event_type);
        }
    }

    Ok(())
}

/// Process incoming message
async fn process_message(
    state: &WebhookState,
    event: &QqMessageEvent,
    config: &crate::channels::QqConfig,
) -> crate::Result<()> {
    // Skip bot messages
    if event.author.bot.unwrap_or(false) {
        return Ok(());
    }

    // Check if user is allowed
    if !is_qq_allowed(&event.author.id, &config.allowed_qqs) {
        warn!("Message from disallowed QQ: {}", event.author.id);
        return Ok(());
    }

    let content = &event.content;
    if content.is_empty() {
        return Ok(());
    }

    let agent = state
        .agent
        .as_ref()
        .ok_or_else(|| crate::error::MantaError::Internal("Agent not available".to_string()))?;

    // Determine conversation ID (DM vs guild channel)
    let conversation_id = if let Some(ref guild_id) = event.guild_id {
        format!("dm:{}", guild_id) // Direct message
    } else {
        event.channel_id.clone().unwrap_or_default() // Guild channel
    };

    // Create incoming message
    let incoming =
        crate::channels::IncomingMessage::new(&event.author.id, &conversation_id, content)
            .with_metadata(
                crate::channels::MessageMetadata::new()
                    .with_extra("message_id", event.id.clone())
                    .with_extra("username", event.author.username.clone())
                    .with_extra("timestamp", event.timestamp.clone()),
            );

    // Process the message
    match agent.process_message(incoming).await {
        Ok(response) => {
            // Send response back via QQ channel
            if let Err(e) = send_response(config, &conversation_id, &response.content).await {
                error!("Failed to send QQ response: {}", e);
            }
        }
        Err(e) => {
            error!("Failed to process QQ message: {}", e);
        }
    }

    Ok(())
}

/// Check if QQ is allowed
fn is_qq_allowed(qq: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    allowed.iter().any(|q| q == qq)
}

/// Send response back to QQ
async fn send_response(
    config: &crate::channels::QqConfig,
    to: &str,
    content: &str,
) -> crate::Result<()> {
    use crate::channels::Channel;

    let channel = crate::channels::QqChannel::new(config.clone());
    let outgoing = crate::channels::OutgoingMessage::new(
        crate::channels::ConversationId(to.to_string()),
        content,
    );

    channel.send(outgoing).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_qq_allowed() {
        let allowed = vec!["123456".to_string(), "789012".to_string()];

        assert!(is_qq_allowed("123456", &allowed));
        assert!(!is_qq_allowed("999999", &allowed));

        // Empty allowed list allows all
        let empty: Vec<String> = vec![];
        assert!(is_qq_allowed("123456", &empty));
    }

    #[test]
    fn test_generate_callback_signature() {
        let token = "test_token";
        let secret = "test_secret";

        let sig = generate_callback_signature(token, secret);
        assert!(!sig.is_empty());

        // Same input should produce same output
        let sig2 = generate_callback_signature(token, secret);
        assert_eq!(sig, sig2);
    }

    #[test]
    fn test_parse_message_event() {
        let json = r#"{
            "id": "msg123",
            "channel_id": "chan456",
            "guild_id": "guild789",
            "content": "Hello!",
            "timestamp": "1234567890",
            "author": {
                "id": "user123",
                "username": "TestUser",
                "bot": false
            },
            "member": {
                "roles": ["role1"],
                "joined_at": "1234567890"
            }
        }"#;

        let event: QqMessageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.id, "msg123");
        assert_eq!(event.author.id, "user123");
        assert_eq!(event.content, "Hello!");
    }

    #[test]
    fn test_parse_webhook_payload() {
        let json = r#"{
            "t": "MESSAGE_CREATE",
            "s": 1,
            "op": 0,
            "d": {
                "id": "msg123",
                "content": "Hello!",
                "author": {
                    "id": "user123",
                    "username": "TestUser"
                }
            }
        }"#;

        let webhook: QqWebhook = serde_json::from_str(json).unwrap();
        assert_eq!(webhook.event_type, Some("MESSAGE_CREATE".to_string()));
        assert!(webhook.data.is_some());
    }
}
