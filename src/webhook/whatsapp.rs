//! WhatsApp Webhook Receiver
//!
//! Handles incoming webhooks from Meta Business API for WhatsApp.
//! Docs: https://developers.facebook.com/docs/whatsapp/cloud-api/guides/set-up-webhooks

use super::WebhookState;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tracing::{debug, error, info, warn};

type HmacSha256 = Hmac<Sha256>;

/// WhatsApp webhook verification query parameters
#[derive(Debug, Deserialize)]
pub struct WhatsappVerificationQuery {
    #[serde(rename = "hub.mode")]
    pub mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    pub verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    pub challenge: Option<String>,
}

/// WhatsApp webhook payload
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WhatsappWebhook {
    pub object: String,
    pub entry: Vec<WhatsappEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WhatsappEntry {
    pub id: String,
    pub changes: Vec<WhatsappChange>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WhatsappChange {
    pub value: WhatsappValue,
    pub field: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WhatsappValue {
    #[serde(rename = "messaging_product")]
    pub messaging_product: String,
    pub metadata: WhatsappMetadata,
    pub contacts: Option<Vec<WhatsappContact>>,
    pub messages: Option<Vec<WhatsappMessage>>,
    pub statuses: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WhatsappMetadata {
    #[serde(rename = "display_phone_number")]
    pub display_phone_number: String,
    #[serde(rename = "phone_number_id")]
    pub phone_number_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WhatsappContact {
    #[serde(rename = "wa_id")]
    pub wa_id: String,
    pub profile: WhatsappProfile,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WhatsappProfile {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WhatsappMessage {
    pub from: String,
    pub id: String,
    pub timestamp: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub text: Option<WhatsappText>,
    pub image: Option<serde_json::Value>,
    pub document: Option<serde_json::Value>,
    pub audio: Option<serde_json::Value>,
    pub location: Option<serde_json::Value>,
    pub context: Option<serde_json::Value>, // For replies
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WhatsappText {
    pub body: String,
}

/// Create WhatsApp webhook router
pub fn whatsapp_webhook_router(state: WebhookState) -> Router {
    Router::new()
        .route("/", get(verify_webhook).post(handle_webhook))
        .with_state(state)
}

/// Verify webhook endpoint (GET)
/// Meta sends a verification challenge when setting up the webhook
async fn verify_webhook(
    State(state): State<WebhookState>,
    Query(query): Query<WhatsappVerificationQuery>,
) -> impl IntoResponse {
    debug!("WhatsApp webhook verification request: {:?}", query);

    // Check if this is a subscription verification
    if query.mode.as_deref() != Some("subscribe") {
        warn!("Invalid hub.mode in verification request");
        return (StatusCode::BAD_REQUEST, "Invalid mode").into_response();
    }

    // Verify the token matches our configured token
    if let Some(ref config) = state.whatsapp_config {
        if query.verify_token.as_deref() != Some(&config.verify_token) {
            warn!("Verify token mismatch");
            return (StatusCode::FORBIDDEN, "Invalid verify token").into_response();
        }
    } else {
        warn!("WhatsApp not configured");
        return (StatusCode::SERVICE_UNAVAILABLE, "WhatsApp not configured").into_response();
    }

    // Return the challenge to confirm verification
    if let Some(challenge) = query.challenge {
        info!("WhatsApp webhook verified successfully");
        (StatusCode::OK, challenge).into_response()
    } else {
        warn!("No challenge provided in verification request");
        (StatusCode::BAD_REQUEST, "No challenge").into_response()
    }
}

/// Handle incoming webhook events (POST)
async fn handle_webhook(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    Json(payload): Json<WhatsappWebhook>,
) -> impl IntoResponse {
    debug!("Received WhatsApp webhook: {:?}", payload);

    // Verify the payload signature if we have the app secret
    if let Some(ref config) = state.whatsapp_config {
        if let Some(signature) = headers.get("x-hub-signature-256") {
            if let Ok(sig_str) = signature.to_str() {
                let body = serde_json::to_string(&payload).unwrap_or_default();
                if !verify_signature(&body, sig_str, &config.access_token) {
                    warn!("Invalid webhook signature");
                    return (StatusCode::FORBIDDEN, "Invalid signature");
                }
            }
        }

        // Process the webhook entries
        for entry in &payload.entry {
            for change in &entry.changes {
                if change.field == "messages" {
                    if let Err(e) = process_messages(&state, &change.value, config).await {
                        error!("Failed to process WhatsApp messages: {}", e);
                    }
                }
            }
        }
    } else {
        warn!("Received WhatsApp webhook but WhatsApp is not configured");
    }

    // Always return 200 OK to acknowledge receipt
    // WhatsApp will retry if it doesn't receive 200
    (StatusCode::OK, "OK")
}

/// Verify webhook signature
fn verify_signature(body: &str, signature: &str, app_secret: &str) -> bool {
    // Signature format: "sha256=<base64_encoded_hmac>"
    let expected_sig = signature.strip_prefix("sha256=");
    if expected_sig.is_none() {
        return false;
    }

    let mut mac = match HmacSha256::new_from_slice(app_secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };

    mac.update(body.as_bytes());
    let result = mac.finalize();
    let computed_sig = hex::encode(result.into_bytes());

    computed_sig == expected_sig.unwrap()
}

/// Process incoming messages
async fn process_messages(
    state: &WebhookState,
    value: &WhatsappValue,
    config: &crate::channels::WhatsappConfig,
) -> crate::Result<()> {
    let agent = state
        .agent
        .as_ref()
        .ok_or_else(|| crate::error::MantaError::Internal("Agent not available".to_string()))?;

    // Process each message
    if let Some(ref messages) = value.messages {
        for msg in messages {
            // Skip non-text messages for now
            if msg.message_type != "text" {
                debug!("Skipping non-text message type: {}", msg.message_type);
                continue;
            }

            let content = msg
                .text
                .as_ref()
                .map(|t| t.body.clone())
                .unwrap_or_default();

            if content.is_empty() {
                continue;
            }

            // Check if number is allowed
            if !is_number_allowed(&msg.from, &config.allowed_numbers) {
                warn!("Message from disallowed number: {}", msg.from);
                continue;
            }

            // Create incoming message
            let incoming = crate::channels::IncomingMessage::new(
                &msg.from,
                &msg.from, // Use sender's phone number as conversation ID
                &content,
            )
            .with_metadata(
                crate::channels::MessageMetadata::new()
                    .with_extra("message_id", msg.id.clone())
                    .with_extra("timestamp", msg.timestamp.clone())
                    .with_extra("phone_number_id", value.metadata.phone_number_id.clone()),
            );

            // Process the message
            match agent.process_message(incoming).await {
                Ok(response) => {
                    // Send response back via the WhatsApp channel
                    if let Err(e) = send_response(config, &msg.from, &response.content).await {
                        error!("Failed to send WhatsApp response: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to process WhatsApp message: {}", e);
                }
            }
        }
    }

    Ok(())
}

/// Check if phone number is allowed
fn is_number_allowed(number: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let normalized = number.trim_start_matches('+').to_string();
    allowed
        .iter()
        .any(|n| n.trim_start_matches('+') == normalized)
}

/// Send response back to WhatsApp
async fn send_response(
    config: &crate::channels::WhatsappConfig,
    to: &str,
    content: &str,
) -> crate::Result<()> {
    use crate::channels::Channel;

    let channel = crate::channels::WhatsappChannel::new(config.clone());
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
    fn test_is_number_allowed() {
        let allowed = vec!["+1234567890".to_string(), "+0987654321".to_string()];

        assert!(is_number_allowed("+1234567890", &allowed));
        assert!(is_number_allowed("1234567890", &allowed));
        assert!(!is_number_allowed("+5555555555", &allowed));

        // Empty allowed list allows all
        let empty: Vec<String> = vec![];
        assert!(is_number_allowed("+1234567890", &empty));
    }

    #[test]
    fn test_verify_signature() {
        let secret = "test_secret";
        let body = r#"{"test":"data"}"#;

        // Compute valid signature
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body.as_bytes());
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));

        assert!(verify_signature(body, &sig, secret));
        assert!(!verify_signature(body, "sha256=invalid", secret));
    }

    #[test]
    fn test_parse_webhook_payload() {
        let json = r#"{
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "12345",
                "changes": [{
                    "value": {
                        "messaging_product": "whatsapp",
                        "metadata": {
                            "display_phone_number": "1234567890",
                            "phone_number_id": "67890"
                        },
                        "contacts": [{
                            "wa_id": "9876543210",
                            "profile": {"name": "Test User"}
                        }],
                        "messages": [{
                            "from": "9876543210",
                            "id": "msg123",
                            "timestamp": "1234567890",
                            "type": "text",
                            "text": {"body": "Hello!"}
                        }]
                    },
                    "field": "messages"
                }]
            }]
        }"#;

        let webhook: WhatsappWebhook = serde_json::from_str(json).unwrap();
        assert_eq!(webhook.object, "whatsapp_business_account");
        assert_eq!(webhook.entry.len(), 1);
        assert_eq!(webhook.entry[0].changes[0].field, "messages");
    }
}
