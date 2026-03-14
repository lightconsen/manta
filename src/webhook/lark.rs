//! Lark/Feishu Webhook Receiver
//!
//! Handles incoming webhooks from ByteDance Lark Open Platform.
//! Docs: https://open.feishu.cn/document/home/event-based-messages/overview

use super::WebhookState;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::{debug, error, info, warn};

type HmacSha256 = Hmac<Sha256>;

/// Lark webhook payload
#[derive(Debug, Deserialize, Clone)]
pub struct LarkWebhook {
    /// UUID for this event
    pub uuid: Option<String>,
    /// Event token
    pub token: Option<String>,
    /// Timestamp
    pub ts: Option<String>,
    /// Event type
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    /// Event data (varies by type)
    pub event: Option<serde_json::Value>,
    /// Challenge for URL verification
    pub challenge: Option<String>,
    /// Encrypted data (if encryption is enabled)
    pub encrypt: Option<String>,
}

/// Lark URL verification response
#[derive(Debug, Serialize)]
pub struct LarkChallengeResponse {
    pub challenge: String,
}

/// Lark message event
#[derive(Debug, Deserialize, Clone)]
pub struct LarkMessageEvent {
    pub sender: LarkSender,
    pub message: LarkMessage,
    #[serde(rename = "message_id")]
    pub message_id: String,
    #[serde(rename = "create_time")]
    pub create_time: String,
    #[serde(rename = "chat_type")]
    pub chat_type: String,
    #[serde(rename = "chat_id")]
    pub chat_id: Option<String>,
    #[serde(rename = "open_chat_id")]
    pub open_chat_id: Option<String>,
    #[serde(rename = "open_id")]
    pub open_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LarkSender {
    #[serde(rename = "sender_id")]
    pub sender_id: LarkSenderId,
    #[serde(rename = "sender_type")]
    pub sender_type: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LarkSenderId {
    #[serde(rename = "open_id")]
    pub open_id: String,
    #[serde(rename = "union_id")]
    pub union_id: Option<String>,
    #[serde(rename = "user_id")]
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LarkMessage {
    #[serde(rename = "message_id")]
    pub message_id: String,
    #[serde(rename = "root_id")]
    pub root_id: Option<String>,
    #[serde(rename = "parent_id")]
    pub parent_id: Option<String>,
    #[serde(rename = "create_time")]
    pub create_time: String,
    #[serde(rename = "chat_id")]
    pub chat_id: String,
    #[serde(rename = "chat_type")]
    pub chat_type: String,
    #[serde(rename = "message_type")]
    pub message_type: String,
    pub content: String,
    pub mentions: Option<Vec<LarkMention>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LarkMention {
    #[serde(rename = "key")]
    pub key: String,
    pub id: LarkMentionId,
    #[serde(rename = "name")]
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LarkMentionId {
    #[serde(rename = "union_id")]
    pub union_id: Option<String>,
    #[serde(rename = "open_id")]
    pub open_id: Option<String>,
    #[serde(rename = "user_id")]
    pub user_id: Option<String>,
}

/// Create Lark webhook router
pub fn lark_webhook_router(state: WebhookState) -> Router {
    Router::new()
        .route("/", post(handle_webhook))
        .with_state(state)
}

/// Handle incoming webhook events
async fn handle_webhook(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    debug!("Received Lark webhook, headers: {:?}", headers);

    let config = match state.lark_config {
        Some(ref c) => c,
        None => {
            warn!("Received Lark webhook but Lark is not configured");
            return (StatusCode::SERVICE_UNAVAILABLE, "Lark not configured").into_response();
        }
    };

    // Get signature headers
    let timestamp = headers
        .get("x-lark-request-timestamp")
        .and_then(|v| v.to_str().ok());
    let nonce = headers
        .get("x-lark-request-nonce")
        .and_then(|v| v.to_str().ok());
    let signature = headers
        .get("x-lark-signature")
        .and_then(|v| v.to_str().ok());

    // Verify signature if present
    if let (Some(ts), Some(nonce), Some(sig)) = (timestamp, nonce, signature) {
        let body_str = String::from_utf8_lossy(&body);
        if !verify_signature(ts, nonce, &body_str, sig, &config.app_secret) {
            warn!("Invalid Lark webhook signature");
            return (StatusCode::FORBIDDEN, "Invalid signature").into_response();
        }
    }

    // Parse the payload
    let payload: LarkWebhook = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to parse Lark webhook: {}", e);
            return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response();
        }
    };

    // Handle URL verification (challenge-response)
    if let Some(challenge) = payload.challenge {
        info!("Handling Lark URL verification challenge");
        let response = LarkChallengeResponse { challenge };
        return (StatusCode::OK, Json(response)).into_response();
    }

    // Process the event
    if let Err(e) = process_event(&state, &payload, config).await {
        error!("Failed to process Lark event: {}", e);
    }

    // Return success response
    (StatusCode::OK, "OK").into_response()
}

/// Verify Lark webhook signature
/// Signature = HMACSHA256(timestamp + nonce + body)
fn verify_signature(
    timestamp: &str,
    nonce: &str,
    body: &str,
    signature: &str,
    secret: &str,
) -> bool {
    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    let data = format!("{}{}{}", timestamp, nonce, body);
    mac.update(data.as_bytes());
    let result = mac.finalize();
    let computed_sig = general_purpose::STANDARD.encode(result.into_bytes());

    computed_sig == signature
}

/// Process Lark event
async fn process_event(
    state: &WebhookState,
    payload: &LarkWebhook,
    config: &crate::channels::LarkConfig,
) -> crate::Result<()> {
    // Check event token matches our verification token
    if let Some(ref token) = payload.token {
        if token != &config.verification_token {
            warn!("Lark event token mismatch");
            return Err(crate::error::MantaError::Validation(
                "Invalid verification token".to_string(),
            ));
        }
    }

    // Handle message events
    if let Some(ref event) = payload.event {
        let event_type = event.get("type").and_then(|v| v.as_str());

        if event_type == Some("message") {
            let message_event: LarkMessageEvent = match serde_json::from_value(event.clone()) {
                Ok(e) => e,
                Err(e) => {
                    error!("Failed to parse message event: {}", e);
                    return Ok(());
                }
            };

            if let Err(e) = process_message(state, &message_event, config).await {
                error!("Failed to process Lark message: {}", e);
            }
        }
    }

    Ok(())
}

/// Process incoming message
async fn process_message(
    state: &WebhookState,
    event: &LarkMessageEvent,
    config: &crate::channels::LarkConfig,
) -> crate::Result<()> {
    // Check if user is allowed
    if !is_user_allowed(&event.sender.sender_id.open_id, &config.allowed_users) {
        warn!(
            "Message from disallowed user: {}",
            event.sender.sender_id.open_id
        );
        return Ok(());
    }

    // Parse message content based on type
    let content = match event.message.message_type.as_str() {
        "text" => parse_text_content(&event.message.content)?,
        _ => {
            debug!(
                "Skipping non-text message type: {}",
                event.message.message_type
            );
            return Ok(());
        }
    };

    if content.is_empty() {
        return Ok(());
    }

    let agent = state
        .agent
        .as_ref()
        .ok_or_else(|| crate::error::MantaError::Internal("Agent not available".to_string()))?;

    // Determine conversation ID (chat for groups, sender for DMs)
    let conversation_id = if event.chat_type == "p2p" {
        event.sender.sender_id.open_id.clone()
    } else {
        event.message.chat_id.clone()
    };

    // Create incoming message
    let incoming = crate::channels::IncomingMessage::new(
        &event.sender.sender_id.open_id,
        &conversation_id,
        &content,
    )
    .with_metadata(
        crate::channels::MessageMetadata::new()
            .with_extra("message_id", event.message.message_id.clone())
            .with_extra("chat_type", event.chat_type.clone())
            .with_extra("message_type", event.message.message_type.clone()),
    );

    // Process the message
    match agent.process_message(incoming).await {
        Ok(response) => {
            // Send response back via Lark channel
            if let Err(e) = send_response(config, &conversation_id, &response.content).await {
                error!("Failed to send Lark response: {}", e);
            }
        }
        Err(e) => {
            error!("Failed to process Lark message: {}", e);
        }
    }

    Ok(())
}

/// Parse text content from Lark message
fn parse_text_content(content: &str) -> crate::Result<String> {
    // Lark text messages are JSON encoded: {"text": "message content"}
    let parsed: serde_json::Value = serde_json::from_str(content)?;
    Ok(parsed
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

/// Check if user is allowed
fn is_user_allowed(user_id: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    allowed.iter().any(|u| u == user_id)
}

/// Send response back to Lark
async fn send_response(
    config: &crate::channels::LarkConfig,
    to: &str,
    content: &str,
) -> crate::Result<()> {
    use crate::channels::Channel;

    let channel = crate::channels::LarkChannel::new(config.clone());
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
    fn test_verify_signature() {
        let timestamp = "1234567890";
        let nonce = "abc123";
        let body = r#"{"test":"data"}"#;
        let secret = "test_secret";

        // Compute valid signature
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        let data = format!("{}{}{}", timestamp, nonce, body);
        mac.update(data.as_bytes());
        let expected_sig = general_purpose::STANDARD.encode(mac.finalize().into_bytes());

        assert!(verify_signature(timestamp, nonce, body, &expected_sig, secret));
        assert!(!verify_signature(timestamp, nonce, body, "invalid", secret));
    }

    #[test]
    fn test_parse_text_content() {
        let content = r#"{"text": "Hello World!"}"#;
        let result = parse_text_content(content).unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_is_user_allowed() {
        let allowed = vec!["user1".to_string(), "user2".to_string()];

        assert!(is_user_allowed("user1", &allowed));
        assert!(!is_user_allowed("user3", &allowed));

        // Empty allowed list allows all
        let empty: Vec<String> = vec![];
        assert!(is_user_allowed("user1", &empty));
    }

    #[test]
    fn test_parse_challenge_payload() {
        let json = r#"{
            "challenge": "test_challenge",
            "token": "test_token"
        }"#;

        let webhook: LarkWebhook = serde_json::from_str(json).unwrap();
        assert_eq!(webhook.challenge, Some("test_challenge".to_string()));
    }

    #[test]
    fn test_parse_message_event() {
        let json = r#"{
            "sender": {
                "sender_id": {"open_id": "user123"},
                "sender_type": "user"
            },
            "message": {
                "message_id": "msg123",
                "create_time": "1234567890",
                "chat_type": "p2p",
                "chat_id": "chat123",
                "message_type": "text",
                "content": "{\"text\":\"Hello\"}"
            },
            "message_id": "msg123",
            "create_time": "1234567890",
            "chat_type": "p2p",
            "chat_id": "chat123",
            "open_id": "user123"
        }"#;

        let event: LarkMessageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.sender.sender_id.open_id, "user123");
        assert_eq!(event.message.message_type, "text");
    }
}
