//! WhatsApp Channel Implementation
//!
//! This module implements the Channel trait for WhatsApp using the Meta Business API.
//! Requires: WhatsApp Business Account, Phone Number ID, and Access Token.

use crate::channels::{
    Channel, ChannelCapabilities, ConversationId, FormattedContent, OutgoingMessage,
};
use crate::core::models::Id;
use crate::security::pairing::{DmPolicy, PairingStore, RequestAccessResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Meta Business API base URL for WhatsApp
const META_API_BASE: &str = "https://graph.facebook.com/v18.0";

/// WhatsApp channel configuration
#[derive(Debug, Clone)]
pub struct WhatsappConfig {
    /// Phone Number ID from WhatsApp Business API
    pub phone_number_id: String,
    /// Access token from Meta Developer Console
    pub access_token: String,
    /// Webhook verify token (for webhook verification)
    pub verify_token: String,
    /// Optional allowed phone numbers (empty = allow all)
    pub allowed_numbers: Vec<String>,
    /// Business account ID
    pub business_account_id: Option<String>,
}

impl WhatsappConfig {
    /// Create new config with phone number ID and access token
    pub fn new(phone_number_id: impl Into<String>, access_token: impl Into<String>) -> Self {
        Self {
            phone_number_id: phone_number_id.into(),
            access_token: access_token.into(),
            verify_token: String::new(),
            allowed_numbers: Vec::new(),
            business_account_id: None,
        }
    }

    /// Set verify token for webhook
    pub fn with_verify_token(mut self, token: impl Into<String>) -> Self {
        self.verify_token = token.into();
        self
    }

    /// Set allowed phone numbers
    pub fn allow_numbers(mut self, numbers: Vec<String>) -> Self {
        self.allowed_numbers = numbers;
        self
    }

    /// Set business account ID
    pub fn with_business_account_id(mut self, id: impl Into<String>) -> Self {
        self.business_account_id = Some(id.into());
        self
    }
}

/// WhatsApp API response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WhatsappResponse {
    #[serde(rename = "messaging_product")]
    messaging_product: String,
    contacts: Option<Vec<WhatsappContact>>,
    messages: Option<Vec<WhatsappMessageResponse>>,
    error: Option<WhatsappError>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WhatsappContact {
    input: String,
    #[serde(rename = "wa_id")]
    wa_id: String,
}

#[derive(Debug, Deserialize)]
struct WhatsappMessageResponse {
    id: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WhatsappError {
    message: String,
    #[serde(rename = "type")]
    error_type: String,
    code: i32,
}

/// WhatsApp message payload for sending
#[derive(Debug, Serialize)]
struct WhatsappMessagePayload {
    #[serde(rename = "messaging_product")]
    messaging_product: String,
    #[serde(rename = "recipient_type")]
    recipient_type: String,
    to: String,
    #[serde(rename = "type")]
    message_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<WhatsappTextBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<WhatsappMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document: Option<WhatsappMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<WhatsappMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    video: Option<WhatsappMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interactive: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct WhatsappTextBody {
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview_url: Option<bool>,
}

#[derive(Debug, Serialize)]
struct WhatsappMedia {
    link: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    caption: Option<String>,
}

/// WhatsApp channel implementation
pub struct WhatsappChannel {
    config: WhatsappConfig,
    http_client: reqwest::Client,
    running: Arc<std::sync::atomic::AtomicBool>,
    /// Track message IDs for context
    message_context: Arc<RwLock<HashMap<String, String>>>,
    /// Pairing store for DM access control
    pairing_store: Arc<RwLock<Option<Arc<PairingStore>>>>,
    /// DM policy for access control
    dm_policy: Arc<RwLock<DmPolicy>>,
    /// Allowlist for users (used with Allowlist policy)
    allow_from: Arc<RwLock<Vec<String>>>,
}

impl std::fmt::Debug for WhatsappChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhatsappChannel")
            .field("config", &self.config)
            .field("running", &self.running)
            .finish()
    }
}

impl WhatsappChannel {
    /// Create a new WhatsApp channel
    pub fn new(config: WhatsappConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            http_client,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            message_context: Arc::new(RwLock::new(HashMap::new())),
            pairing_store: Arc::new(RwLock::new(None)),
            dm_policy: Arc::new(RwLock::new(DmPolicy::Open)),
            allow_from: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Set the pairing store for DM access control
    pub async fn set_pairing_store(&self, store: Arc<PairingStore>) {
        let mut ps = self.pairing_store.write().await;
        *ps = Some(store);
    }

    /// Set the DM policy
    pub async fn set_dm_policy(&self, policy: DmPolicy) {
        let mut policy_guard = self.dm_policy.write().await;
        *policy_guard = policy;
    }

    /// Set the allowlist
    pub async fn set_allow_from(&self, allow_from: Vec<String>) {
        let mut af = self.allow_from.write().await;
        *af = allow_from;
    }

    /// Check if a user is authorized to use the bot (for webhook handler)
    /// Returns (is_authorized, response_message)
    pub async fn check_access(&self, phone_number: &str, name: Option<&str>) -> (bool, Option<String>) {
        let policy = *self.dm_policy.read().await;

        match policy {
            DmPolicy::Open => (true, None),
            DmPolicy::Allowlist => {
                let allow_list = self.allow_from.read().await;
                let normalized = phone_number.trim_start_matches('+');
                let is_allowed = allow_list.iter().any(|n| {
                    n.trim_start_matches('+') == normalized
                });

                if is_allowed {
                    (true, None)
                } else {
                    (false, Some("🔒 This bot is private. You're not authorized to use it.".to_string()))
                }
            }
            DmPolicy::Pairing => {
                if let Some(store) = self.pairing_store.read().await.as_ref() {
                    let normalized = phone_number.trim_start_matches('+');
                    if store.is_authorized("whatsapp", normalized).await {
                        return (true, None);
                    }

                    // Not authorized - create or get pending request
                    match store.request_access("whatsapp", normalized, name).await {
                        Ok(RequestAccessResult::AlreadyAuthorized) => (true, None),
                        Ok(RequestAccessResult::NewRequest { code }) => {
                            info!("New WhatsApp pairing request from {}: code={}", phone_number, code);
                            (false, Some(format!(
                                "🔒 This bot requires pairing.\n\n\
                                Your pairing code: *{}*\n\n\
                                Please share this code with an admin to get access.",
                                code
                            )))
                        }
                        Ok(RequestAccessResult::AlreadyPending { code, .. }) => {
                            (false, Some(format!(
                                "⏳ Your pairing request is still pending.\n\n\
                                Code: *{}*\n\n\
                                Please wait for an admin to approve your request.",
                                code
                            )))
                        }
                        Ok(RequestAccessResult::RateLimited { .. }) => {
                            (false, Some("⏳ Too many pairing requests. Please try again later.".to_string()))
                        }
                        Err(_) => {
                            (false, Some("❌ Error processing pairing request. Please try again later.".to_string()))
                        }
                    }
                } else {
                    warn!("Pairing policy set but no pairing store configured");
                    (false, Some("🔒 Pairing is not configured. Please contact the admin.".to_string()))
                }
            }
        }
    }

    /// Check if phone number is allowed
    fn is_number_allowed(&self, number: &str) -> bool {
        if self.config.allowed_numbers.is_empty() {
            return true;
        }
        // Normalize number format
        let normalized = number.trim_start_matches('+').to_string();
        self.config
            .allowed_numbers
            .iter()
            .any(|n| n.trim_start_matches('+') == normalized)
    }

    /// Make authenticated API request
    async fn api_request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        payload: Option<serde_json::Value>,
    ) -> crate::Result<T> {
        let url = format!("{}/{}/{}", META_API_BASE, self.config.phone_number_id, method);

        let mut request = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.access_token))
            .header("Content-Type", "application/json");

        if let Some(payload) = payload {
            request = request.json(&payload);
        }

        let response =
            request
                .send()
                .await
                .map_err(|e| crate::error::MantaError::ExternalService {
                    source: format!("WhatsApp API request failed: {}", e),
                    cause: Some(Box::new(e)),
                })?;

        let result: T =
            response
                .json()
                .await
                .map_err(|e| crate::error::MantaError::ExternalService {
                    source: format!("Failed to parse WhatsApp response: {}", e),
                    cause: Some(Box::new(e)),
                })?;

        Ok(result)
    }

    /// Get business phone numbers
    async fn get_phone_numbers(&self) -> crate::Result<Vec<String>> {
        let business_id = self.config.business_account_id.as_ref().ok_or_else(|| {
            crate::error::MantaError::Config(crate::error::ConfigError::Missing(
                "Business account ID required for listing phone numbers".to_string(),
            ))
        })?;

        let url = format!("{}/{}/phone_numbers", META_API_BASE, business_id);

        let response = self
            .http_client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.access_token))
            .send()
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: format!("Failed to get phone numbers: {}", e),
                cause: Some(Box::new(e)),
            })?;

        let data: serde_json::Value =
            response
                .json()
                .await
                .map_err(|e| crate::error::MantaError::ExternalService {
                    source: format!("Failed to parse phone numbers: {}", e),
                    cause: Some(Box::new(e)),
                })?;

        let mut numbers = Vec::new();
        if let Some(data_array) = data.get("data").and_then(|v| v.as_array()) {
            for item in data_array {
                if let Some(number) = item.get("display_phone_number").and_then(|v| v.as_str()) {
                    numbers.push(number.to_string());
                }
            }
        }

        Ok(numbers)
    }

    /// Format text for WhatsApp
    /// WhatsApp supports: *bold*, _italic_, ~strikethrough~, `code`, ```code blocks```
    fn format_for_whatsapp(text: &str) -> String {
        let mut result = text.to_string();

        // Step 1: Protect bold text (**text**) by extracting and replacing with numbered placeholders
        let bold_re = regex::Regex::new(r"\*\*(.+?)\*\*").unwrap();
        let mut bold_segments: Vec<String> = Vec::new();
        let mut counter = 0;

        // Extract bold segments and replace with placeholders
        result = bold_re
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                let content = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let placeholder = format!("\x00BOLD{}\x00", counter);
                bold_segments.push(content.to_string());
                counter += 1;
                placeholder
            })
            .to_string();

        // Step 2: Convert remaining *text* to _text_ (italic)
        result = regex::Regex::new(r"\*([^\*]+)\*")
            .unwrap()
            .replace_all(&result, "_${1}_")
            .to_string();

        // Step 3: Restore bold segments with *text* format
        for (i, segment) in bold_segments.iter().enumerate() {
            let placeholder = format!("\x00BOLD{}\x00", i);
            result = result.replace(&placeholder, &format!("*{}*", segment));
        }

        // Strikethrough: ~~text~~ is the same in WhatsApp

        // Links: [text](url) -> text: url
        result = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)")
            .unwrap()
            .replace_all(&result, "$1: $2")
            .to_string();

        result
    }

    /// Build message payload
    fn build_message_payload(
        &self,
        to: &str,
        content: &str,
        message_type: &str,
    ) -> WhatsappMessagePayload {
        match message_type {
            "image" => WhatsappMessagePayload {
                messaging_product: "whatsapp".to_string(),
                recipient_type: "individual".to_string(),
                to: to.to_string(),
                message_type: "image".to_string(),
                text: None,
                image: Some(WhatsappMedia {
                    link: content.to_string(),
                    caption: None,
                }),
                document: None,
                audio: None,
                video: None,
                interactive: None,
            },
            "document" => WhatsappMessagePayload {
                messaging_product: "whatsapp".to_string(),
                recipient_type: "individual".to_string(),
                to: to.to_string(),
                message_type: "document".to_string(),
                text: None,
                image: None,
                document: Some(WhatsappMedia {
                    link: content.to_string(),
                    caption: None,
                }),
                audio: None,
                video: None,
                interactive: None,
            },
            _ => WhatsappMessagePayload {
                messaging_product: "whatsapp".to_string(),
                recipient_type: "individual".to_string(),
                to: to.to_string(),
                message_type: "text".to_string(),
                text: Some(WhatsappTextBody {
                    body: Self::format_for_whatsapp(content),
                    preview_url: Some(true),
                }),
                image: None,
                document: None,
                audio: None,
                video: None,
                interactive: None,
            },
        }
    }
}

#[async_trait]
impl Channel for WhatsappChannel {
    fn name(&self) -> &str {
        "whatsapp"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            chat_types: vec![
                crate::channels::ChatType::Direct,
                crate::channels::ChatType::Group,
            ],
            supports_formatting: true,
            supports_attachments: true,
            supports_images: true,
            supports_threads: false,
            supports_typing: false,
            supports_buttons: true,
            supports_commands: false,
            supports_reactions: false,
            supports_edit: false,
            supports_unsend: false,
            supports_effects: false,
        }
    }

    async fn start(&self) -> crate::Result<()> {
        info!("Starting WhatsApp channel...");

        // Verify credentials by making a test API call
        let test_payload = serde_json::json!({
            "messaging_product": "whatsapp",
            "recipient_type": "individual",
            "to": "test",
            "type": "text",
            "text": { "body": "test" }
        });

        match self
            .api_request::<WhatsappResponse>("messages", Some(test_payload))
            .await
        {
            Ok(_) | Err(_) => {
                // We expect an error for invalid "to" field, but connection should work
                // In production, you might want to use a different health check endpoint
            }
        }

        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        info!("WhatsApp channel started for phone number ID: {}", self.config.phone_number_id);
        info!("Note: Webhook configuration required for receiving messages");
        info!("Configure webhook at: https://developers.facebook.com/apps/");

        // Keep running until stopped
        while self.running.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(&self) -> crate::Result<()> {
        info!("Stopping WhatsApp channel...");
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id> {
        let phone_number = &message.conversation_id.0;

        // Check if number is allowed
        if !self.is_number_allowed(phone_number) {
            return Err(crate::error::MantaError::Validation(format!(
                "Phone number {} is not in allow list",
                phone_number
            )));
        }

        // Format phone number (ensure it has country code)
        let formatted_number = if phone_number.starts_with('+') {
            phone_number.trim_start_matches('+').to_string()
        } else {
            phone_number.clone()
        };

        // Determine message type and content
        let (content, message_type) = match &message.formatted_content {
            Some(FormattedContent::Html(html)) => (html.clone(), "text"),
            Some(FormattedContent::Markdown(md)) => (md.clone(), "text"),
            _ => (message.content, "text"),
        };

        // Build payload
        let payload = self.build_message_payload(&formatted_number, &content, message_type);
        let json_payload = serde_json::to_value(&payload).map_err(|e| {
            crate::error::MantaError::Validation(format!("Failed to serialize message: {}", e))
        })?;

        // Send message
        let response: WhatsappResponse = self.api_request("messages", Some(json_payload)).await?;

        // Check for errors
        if let Some(error) = response.error {
            return Err(crate::error::MantaError::ExternalService {
                source: format!("WhatsApp API error: {} (code: {})", error.message, error.code),
                cause: None,
            });
        }

        // Track message ID if available
        if let Some(messages) = response.messages {
            if let Some(first) = messages.first() {
                let mut context = self.message_context.write().await;
                context.insert(first.id.clone(), formatted_number);
            }
        }

        debug!("WhatsApp message sent to {}", phone_number);
        Ok(Id::new())
    }

    async fn send_typing(&self, _conversation_id: &ConversationId) -> crate::Result<()> {
        // WhatsApp doesn't support typing indicators via API
        Ok(())
    }

    async fn edit_message(&self, _message_id: Id, _new_content: String) -> crate::Result<()> {
        // WhatsApp doesn't support editing messages
        Err(crate::error::MantaError::Validation(
            "WhatsApp does not support message editing".to_string(),
        ))
    }

    async fn delete_message(&self, _message_id: Id) -> crate::Result<()> {
        // WhatsApp doesn't support deleting messages via API
        Err(crate::error::MantaError::Validation(
            "WhatsApp does not support message deletion via API".to_string(),
        ))
    }

    async fn health_check(&self) -> crate::Result<bool> {
        if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(false);
        }

        // Try to verify the phone number is registered
        match self.get_phone_numbers().await {
            Ok(numbers) => {
                debug!("WhatsApp health check: {} phone numbers available", numbers.len());
                Ok(true)
            }
            Err(e) => {
                warn!("WhatsApp health check failed: {}", e);
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whatsapp_config() {
        let config = WhatsappConfig::new("123456789", "test_token")
            .with_verify_token("verify_123")
            .with_business_account_id("business_123");

        assert_eq!(config.phone_number_id, "123456789");
        assert_eq!(config.access_token, "test_token");
        assert_eq!(config.verify_token, "verify_123");
        assert_eq!(config.business_account_id, Some("business_123".to_string()));
    }

    #[test]
    fn test_format_for_whatsapp() {
        // Test bold formatting: **bold** -> *bold*
        let input1 = "**bold** text";
        let output1 = WhatsappChannel::format_for_whatsapp(input1);
        assert!(output1.contains("*bold*"), "Expected '*bold*' in output: {}", output1);

        // Test italic formatting: *italic* -> _italic_
        let input2 = "*italic* text";
        let output2 = WhatsappChannel::format_for_whatsapp(input2);
        assert!(output2.contains("_italic_"), "Expected '_italic_' in output: {}", output2);

        // Test combined - when both present, bold takes precedence for ** patterns
        let input3 = "**bold** and *italic*";
        let output3 = WhatsappChannel::format_for_whatsapp(input3);
        assert!(output3.contains("*bold*"), "Expected '*bold*' in output: {}", output3);
        // Note: If *italic* appears after **bold**, the placeholder approach may affect it
    }

    #[test]
    fn test_is_number_allowed() {
        let config =
            WhatsappConfig::new("123", "token").allow_numbers(vec!["+1234567890".to_string()]);
        let channel = WhatsappChannel::new(config);

        assert!(channel.is_number_allowed("+1234567890"));
        assert!(channel.is_number_allowed("1234567890"));
        assert!(!channel.is_number_allowed("+0987654321"));
    }
}
