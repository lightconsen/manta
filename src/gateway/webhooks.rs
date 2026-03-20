//! Webhook Receivers - Public Tier
//!
//! These endpoints are publicly accessible for receiving callbacks from
//! external channel providers (WhatsApp, Telegram, Feishu, etc.).
//! Security is handled via HMAC signature verification per-channel.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use super::GatewayState;

/// Query params for webhook verification (used by some platforms)
#[derive(Debug, Deserialize)]
pub struct WebhookVerifyQuery {
    /// Challenge token for verification
    pub hub_challenge: Option<String>,
    /// Verify token sent by platform
    pub hub_verify_token: Option<String>,
    /// Mode (subscribe/unsubscribe)
    pub hub_mode: Option<String>,
}

/// Generic webhook response
#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    pub success: bool,
    pub message: String,
}

/// Session mapping for webhook-based channels (platform_id -> session_uuid)
/// This provides UUID-based sessions with /new command support
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Get or create a session UUID for a platform user
async fn get_or_create_session(
    sessions: &RwLock<HashMap<String, String>>,
    platform_key: &str,
) -> String {
    {
        let map = sessions.read().await;
        if let Some(session_id) = map.get(platform_key) {
            return session_id.clone();
        }
    }
    // Create new session
    let new_session = uuid::Uuid::new_v4().to_string();
    let mut map = sessions.write().await;
    map.insert(platform_key.to_string(), new_session.clone());
    new_session
}

/// Reset session for a platform user (when /new is used)
async fn reset_session(sessions: &RwLock<HashMap<String, String>>, platform_key: &str) -> String {
    let new_session = uuid::Uuid::new_v4().to_string();
    let mut map = sessions.write().await;
    map.insert(platform_key.to_string(), new_session.clone());
    new_session
}

/// Create the public webhook router
pub fn create_webhook_router(state: Arc<GatewayState>) -> Router {
    Router::new()
        // WhatsApp Business API webhooks
        .route("/webhooks/whatsapp", post(whatsapp_webhook_handler))
        .route("/webhooks/whatsapp/verify", get(whatsapp_verify_handler))
        // Telegram Bot API webhooks
        .route("/webhooks/telegram/:token", post(telegram_webhook_handler))
        // Feishu/Lark webhooks
        .route("/webhooks/feishu", post(feishu_webhook_handler))
        // Generic webhook for custom integrations
        .route("/webhooks/:channel", post(generic_webhook_handler))
        .with_state(state)
}

/// Verify WhatsApp webhook subscription (GET request for verification)
async fn whatsapp_verify_handler(
    Query(query): Query<WebhookVerifyQuery>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    info!("WhatsApp webhook verification request");

    // Get verify token from config
    let expected_token = {
        let config = state.config.read().await;
        config
            .channels
            .get("whatsapp")
            .and_then(|c| c.credentials.get("verify_token"))
            .cloned()
    };

    match (query.hub_mode.as_deref(), query.hub_verify_token) {
        (Some("subscribe"), Some(token)) => {
            if expected_token.map(|t| t == token).unwrap_or(true) {
                // Return the challenge
                if let Some(challenge) = query.hub_challenge {
                    info!("WhatsApp webhook verified successfully");
                    return (StatusCode::OK, challenge).into_response();
                }
            }
            warn!("WhatsApp webhook verification failed: invalid token");
            StatusCode::FORBIDDEN.into_response()
        }
        _ => {
            warn!("WhatsApp webhook verification: invalid request");
            StatusCode::BAD_REQUEST.into_response()
        }
    }
}

/// Handle incoming WhatsApp messages with HMAC-SHA256 signature verification
async fn whatsapp_webhook_handler(
    headers: HeaderMap,
    State(state): State<Arc<GatewayState>>,
    body: Bytes,
) -> impl IntoResponse {
    info!("Received WhatsApp webhook");

    // Get HMAC secret from config
    let hmac_secret = {
        let config = state.config.read().await;
        config
            .channels
            .get("whatsapp")
            .and_then(|c| c.credentials.get("app_secret"))
            .cloned()
    };

    // Verify HMAC signature if secret is configured
    if let Some(secret) = hmac_secret {
        let signature = headers
            .get("x-hub-signature-256")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.strip_prefix("sha256=").unwrap_or(s));

        if let Some(sig) = signature {
            if !verify_hmac_sha256(&secret, &body, sig) {
                warn!("WhatsApp webhook: invalid HMAC signature");
                return (StatusCode::UNAUTHORIZED, "Invalid signature").into_response();
            }
            debug!("WhatsApp webhook: HMAC signature verified");
        } else {
            warn!("WhatsApp webhook: missing signature");
            return (StatusCode::UNAUTHORIZED, "Missing signature").into_response();
        }
    }

    // Parse the webhook payload
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to parse WhatsApp webhook: {}", e);
            return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response();
        }
    };

    // Process webhook entries
    if let Some(entries) = payload.get("entry").and_then(|e| e.as_array()) {
        for entry in entries {
            if let Some(changes) = entry.get("changes").and_then(|c| c.as_array()) {
                for change in changes {
                    if let Some(messages) = change
                        .get("value")
                        .and_then(|v| v.get("messages"))
                        .and_then(|m| m.as_array())
                    {
                        for msg in messages {
                            if let (Some(from), Some(text_body)) = (
                                msg.get("from").and_then(|f| f.as_str()),
                                msg.get("text")
                                    .and_then(|t| t.get("body"))
                                    .and_then(|b| b.as_str()),
                            ) {
                                info!(
                                    "WhatsApp message from {}: {}",
                                    from,
                                    &text_body[..text_body.len().min(50)]
                                );

                                // Handle /new command to reset session
                                let platform_key = format!("whatsapp:{}", from);
                                let session_id = if text_body.trim() == "/new" {
                                    let new_session =
                                        reset_session(&state.webhook_sessions, &platform_key).await;
                                    info!(
                                        "🆕 New WhatsApp session started for {}: {}",
                                        from, new_session
                                    );
                                    // Send confirmation message back (would need channel.send here)
                                    new_session
                                } else {
                                    // Get or create session UUID
                                    get_or_create_session(&state.webhook_sessions, &platform_key)
                                        .await
                                };

                                // Store session mapping for response routing
                                {
                                    let mut sessions = state.session_channels.write().await;
                                    sessions.insert(
                                        session_id.clone(),
                                        ("whatsapp".to_string(), from.to_string()),
                                    );
                                }

                                // Queue message for processing
                                let queued_msg = super::QueuedMessage {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    channel: "whatsapp".to_string(),
                                    user_id: from.to_string(),
                                    content: text_body.to_string(),
                                    session_id, // Use UUID session
                                    timestamp: chrono::Utc::now(),
                                };

                                if let Err(e) = state.message_queue.send(queued_msg).await {
                                    error!("Failed to queue WhatsApp message: {}", e);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Json(WebhookResponse {
        success: true,
        message: "Webhook received".to_string(),
    })
    .into_response()
}

/// Telegram webhook payload
#[derive(Debug, Deserialize)]
struct TelegramUpdate {
    update_id: i64,
    message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    message_id: i64,
    from: Option<TelegramUser>,
    chat: TelegramChat,
    date: i64,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramUser {
    id: i64,
    first_name: String,
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChat {
    id: i64,
    #[serde(rename = "type")]
    chat_type: String,
}

/// Handle Telegram webhook with token-based verification
async fn telegram_webhook_handler(
    Path(token): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(update): Json<TelegramUpdate>,
) -> impl IntoResponse {
    // Verify webhook token from URL path
    let expected_token = {
        let config = state.config.read().await;
        config
            .channels
            .get("telegram")
            .and_then(|c| c.credentials.get("webhook_token"))
            .cloned()
    };

    if let Some(expected) = expected_token {
        if expected != token {
            warn!("Telegram webhook: invalid token");
            return (StatusCode::UNAUTHORIZED, "Invalid token").into_response();
        }
        debug!("Telegram webhook: token verified");
    }

    // Process the update
    if let Some(message) = update.message {
        if let Some(text) = message.text {
            let user_id = message
                .from
                .as_ref()
                .map(|u| u.id.to_string())
                .unwrap_or_default();
            let chat_id = message.chat.id.to_string();

            info!(
                "Telegram message from {}: {}",
                user_id,
                text.chars().take(50).collect::<String>()
            );

            // Queue message
            let queued_msg = super::QueuedMessage {
                id: uuid::Uuid::new_v4().to_string(),
                channel: "telegram".to_string(),
                user_id: user_id.clone(),
                content: text,
                session_id: format!("telegram:{}", chat_id),
                timestamp: chrono::Utc::now(),
            };

            if let Err(e) = state.message_queue.send(queued_msg).await {
                error!("Failed to queue Telegram message: {}", e);
            }
        }
    }

    Json(WebhookResponse {
        success: true,
        message: "OK".to_string(),
    })
    .into_response()
}

/// Handle Feishu/Lark webhook with signature verification
async fn feishu_webhook_handler(
    headers: HeaderMap,
    State(state): State<Arc<GatewayState>>,
    body: Bytes,
) -> impl IntoResponse {
    info!("Received Feishu webhook");

    // Get signature info from headers
    let signature = headers
        .get("x-lark-signature")
        .and_then(|v| v.to_str().ok());

    let timestamp = headers
        .get("x-lark-request-timestamp")
        .and_then(|v| v.to_str().ok());

    let nonce = headers
        .get("x-lark-request-nonce")
        .and_then(|v| v.to_str().ok());

    let secret = {
        let config = state.config.read().await;
        config
            .channels
            .get("feishu")
            .and_then(|c| c.credentials.get("webhook_secret"))
            .cloned()
    };

    // Verify signature if secret and headers are present
    if let (Some(secret), Some(sig), Some(ts), Some(nonce)) = (secret, signature, timestamp, nonce)
    {
        if !verify_feishu_signature(&secret, ts, nonce, &body, sig) {
            warn!("Feishu webhook: invalid signature");
            return (StatusCode::UNAUTHORIZED, "Invalid signature").into_response();
        }
        debug!("Feishu webhook: signature verified");
    }

    // Parse the payload
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to parse Feishu webhook: {}", e);
            return (StatusCode::BAD_REQUEST, "Invalid JSON").into_response();
        }
    };

    // Check if this is a challenge request (initial verification)
    if let Some(challenge) = payload.get("challenge").and_then(|v| v.as_str()) {
        return Json(serde_json::json!({
            "challenge": challenge
        }))
        .into_response();
    }

    // Extract message content from event
    if let Some(event) = payload.get("event") {
        if let (Some(message), Some(sender)) = (event.get("message"), event.get("sender")) {
            if let (Some(content), Some(sender_id)) = (
                message.get("content").and_then(|c| c.get("text")),
                sender.get("sender_id").and_then(|s| s.get("open_id")),
            ) {
                let text = content.as_str().unwrap_or_default();
                let user_id = sender_id.as_str().unwrap_or_default();

                info!(
                    "Feishu message from {}: {}",
                    user_id,
                    text.chars().take(50).collect::<String>()
                );

                // Handle /new command to reset session
                let platform_key = format!("feishu:{}", user_id);
                let session_id = if text.trim() == "/new" {
                    let new_session = reset_session(&state.webhook_sessions, &platform_key).await;
                    info!("🆕 New Feishu session started for {}: {}", user_id, new_session);
                    new_session
                } else {
                    // Get or create session UUID
                    get_or_create_session(&state.webhook_sessions, &platform_key).await
                };

                // Store session mapping for response routing
                {
                    let mut sessions = state.session_channels.write().await;
                    sessions
                        .insert(session_id.clone(), ("feishu".to_string(), user_id.to_string()));
                }

                // Queue message
                let queued_msg = super::QueuedMessage {
                    id: uuid::Uuid::new_v4().to_string(),
                    channel: "feishu".to_string(),
                    user_id: user_id.to_string(),
                    content: text.to_string(),
                    session_id, // Use UUID session
                    timestamp: chrono::Utc::now(),
                };

                if let Err(e) = state.message_queue.send(queued_msg).await {
                    error!("Failed to queue Feishu message: {}", e);
                }
            }
        }
    }

    Json(WebhookResponse {
        success: true,
        message: "OK".to_string(),
    })
    .into_response()
}

/// Generic webhook handler for custom integrations with HMAC verification
async fn generic_webhook_handler(
    Path(channel): Path<String>,
    headers: HeaderMap,
    State(state): State<Arc<GatewayState>>,
    body: Bytes,
) -> impl IntoResponse {
    info!("Received generic webhook for channel: {}", channel);

    // Get channel config
    let config = state.config.read().await;
    let channel_config = config.channels.get(&channel);

    if channel_config.is_none() {
        return (StatusCode::NOT_FOUND, "Channel not configured").into_response();
    }

    let channel_config = channel_config.unwrap();

    if !channel_config.enabled {
        return (StatusCode::SERVICE_UNAVAILABLE, "Channel disabled").into_response();
    }

    // Get webhook secret if configured
    let secret = channel_config.credentials.get("webhook_secret");

    // Verify HMAC if secret is set
    if let Some(secret) = secret {
        let signature = headers
            .get("x-signature")
            .or_else(|| headers.get("x-hub-signature-256"))
            .and_then(|v| v.to_str().ok())
            .map(|s| s.strip_prefix("sha256=").unwrap_or(s));

        if let Some(sig) = signature {
            if !verify_hmac_sha256(secret, &body, sig) {
                warn!("{} webhook: invalid HMAC signature", channel);
                return (StatusCode::UNAUTHORIZED, "Invalid signature").into_response();
            }
            debug!("{} webhook: HMAC signature verified", channel);
        } else {
            warn!("{} webhook: missing signature", channel);
            return (StatusCode::UNAUTHORIZED, "Missing signature").into_response();
        }
    }

    // Parse generic JSON payload
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => {
            // Try to parse as plain text
            serde_json::json!({
                "text": String::from_utf8_lossy(&body)
            })
        }
    };

    // Extract user ID and message content
    let user_id = payload
        .get("user_id")
        .or_else(|| payload.get("from"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let content = payload
        .get("message")
        .or_else(|| payload.get("text"))
        .or_else(|| payload.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    if !content.is_empty() {
        // Handle /new command to reset session
        let platform_key = format!("{}:{}", channel, user_id);
        let session_id = if content.trim() == "/new" {
            let new_session = reset_session(&state.webhook_sessions, &platform_key).await;
            info!("🆕 New {} session started for {}: {}", channel, user_id, new_session);
            new_session
        } else {
            // Get or create session UUID
            get_or_create_session(&state.webhook_sessions, &platform_key).await
        };

        // Store session mapping for response routing
        {
            let mut sessions = state.session_channels.write().await;
            sessions.insert(session_id.clone(), (channel.clone(), user_id.clone()));
        }

        // Queue message
        let queued_msg = super::QueuedMessage {
            id: uuid::Uuid::new_v4().to_string(),
            channel: channel.clone(),
            user_id: user_id.clone(),
            content,
            session_id, // Use UUID session
            timestamp: chrono::Utc::now(),
        };

        drop(config); // Release read lock before await

        if let Err(e) = state.message_queue.send(queued_msg).await {
            error!("Failed to queue {} message: {}", channel, e);
        }
    }

    Json(WebhookResponse {
        success: true,
        message: "Webhook received".to_string(),
    })
    .into_response()
}

/// Verify HMAC-SHA256 signature
///
/// Used by WhatsApp and generic webhooks
fn verify_hmac_sha256(secret: &str, body: &[u8], expected_sig: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    let mut mac = match HmacSha256::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            warn!("Failed to create HMAC from secret");
            return false;
        }
    };

    mac.update(body);
    let result = mac.finalize();
    let computed_sig = hex::encode(result.into_bytes());

    // Constant-time comparison to prevent timing attacks
    use subtle::ConstantTimeEq;
    computed_sig
        .as_bytes()
        .ct_eq(expected_sig.as_bytes())
        .into()
}

/// Verify Feishu/Lark signature
///
/// Feishu uses a custom signature algorithm:
/// SHA256(timestamp + nonce + secret + body)
fn verify_feishu_signature(
    secret: &str,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
    expected_sig: &str,
) -> bool {
    use sha2::{Digest, Sha256};

    // Feishu signature: SHA256(timestamp + nonce + secret + body)
    let body_str = String::from_utf8_lossy(body);
    let sign_string = format!("{}{}{}{}", timestamp, nonce, secret, body_str);

    let mut hasher = Sha256::new();
    hasher.update(sign_string.as_bytes());
    let computed_sig = hex::encode(hasher.finalize());

    // Constant-time comparison to prevent timing attacks
    use subtle::ConstantTimeEq;
    computed_sig
        .as_bytes()
        .ct_eq(expected_sig.as_bytes())
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hmac_sha256_verification() {
        let secret = "test_secret";
        let body = b"test message";

        // Compute expected signature
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let expected_sig = hex::encode(mac.finalize().into_bytes());

        // Verify signature
        assert!(verify_hmac_sha256(secret, body, &expected_sig));

        // Verify wrong signature fails
        assert!(!verify_hmac_sha256(secret, body, "invalid_sig"));
    }

    #[test]
    fn test_feishu_signature_verification() {
        let secret = "test_secret";
        let timestamp = "1234567890";
        let nonce = "abc123";
        let body = b"test message";

        // Compute expected signature
        use sha2::{Digest, Sha256};
        let body_str = String::from_utf8_lossy(body);
        let sign_string = format!("{}{}{}{}", timestamp, nonce, secret, body_str);
        let mut hasher = Sha256::new();
        hasher.update(sign_string.as_bytes());
        let expected_sig = hex::encode(hasher.finalize());

        // Verify signature
        assert!(verify_feishu_signature(secret, timestamp, nonce, body, &expected_sig));

        // Verify wrong signature fails
        assert!(!verify_feishu_signature(secret, timestamp, nonce, body, "invalid_sig"));
    }
}
