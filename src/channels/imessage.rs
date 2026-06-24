//! iMessage Channel Implementation
//!
//! This module implements the Channel trait for Apple iMessage using
//! the BlueBubbles server REST API.
//!
//! Requires: BlueBubbles server running (macOS with Messages.app automation)
//!   Default endpoint: http://localhost:3000
//!
//! BlueBubbles provides:
//! - REST API for sending/receiving messages
//! - WebSocket for real-time message events
//! - Contact and chat management

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use crate::channels::{
    Channel, ChannelCapabilities, ChatType, ConversationId, FormattedContent, IncomingMessage,
    OutgoingMessage,
};
use crate::core::models::Id;
use crate::security::pairing::{DmPolicy, PairingStore, RequestAccessResult};

/// Default BlueBubbles server URL
const DEFAULT_BLUEBUBBLES_URL: &str = "http://localhost:3000";

#[allow(clippy::unwrap_used)]
static RE_HTML_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new("<[^>]+>").unwrap());

/// iMessage channel configuration
#[derive(Debug, Clone)]
pub struct ImessageConfig {
    /// BlueBubbles server URL
    pub server_url: String,
    /// BlueBubbles API password (if configured)
    pub api_password: Option<String>,
    /// Optional allowed handles (empty = allow all)
    pub allowed_handles: Vec<String>,
    /// Message handler for incoming messages
    pub message_tx: Option<mpsc::UnboundedSender<IncomingMessage>>,
}

impl ImessageConfig {
    /// Create new config with server URL
    pub fn new() -> Self {
        Self {
            server_url: DEFAULT_BLUEBUBBLES_URL.to_string(),
            api_password: None,
            allowed_handles: Vec::new(),
            message_tx: None,
        }
    }

    /// Set custom server URL
    pub fn with_server_url(mut self, url: impl Into<String>) -> Self {
        self.server_url = url.into();
        self
    }

    /// Set API password
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.api_password = Some(password.into());
        self
    }

    /// Set allowed handles (phone numbers or email addresses)
    pub fn allow_handles(mut self, handles: Vec<String>) -> Self {
        self.allowed_handles = handles;
        self
    }

    /// Set message handler
    pub fn with_message_handler(mut self, tx: mpsc::UnboundedSender<IncomingMessage>) -> Self {
        self.message_tx = Some(tx);
        self
    }
}

impl Default for ImessageConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// BlueBubbles send message request
#[derive(Debug, Serialize)]
struct BbSendRequest {
    #[serde(rename = "chatGuid", skip_serializing_if = "Option::is_none")]
    chat_guid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    message: String,
    #[serde(rename = "method", skip_serializing_if = "Option::is_none")]
    method: Option<String>,
}

/// BlueBubbles API response wrapper
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BbResponse<T> {
    status: i32,
    message: String,
    #[serde(default)]
    data: Option<T>,
}

/// BlueBubbles message data
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BbMessage {
    #[serde(rename = "guid")]
    guid: String,
    #[serde(rename = "text")]
    text: Option<String>,
    #[serde(rename = "handle")]
    handle: Option<BbHandle>,
    #[serde(rename = "date")]
    date: Option<String>,
    #[serde(rename = "chatGuid")]
    chat_guid: Option<String>,
}

/// BlueBubbles handle info
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BbHandle {
    #[serde(rename = "address")]
    address: String,
}

/// iMessage channel implementation
pub struct ImessageChannel {
    config: ImessageConfig,
    http_client: reqwest::Client,
    running: Arc<std::sync::atomic::AtomicBool>,
    /// Track message IDs: internal -> BlueBubbles GUID
    message_map: Arc<RwLock<HashMap<String, String>>>,
    /// Pairing store
    pairing_store: Arc<RwLock<Option<Arc<PairingStore>>>>,
    /// DM policy
    dm_policy: Arc<RwLock<DmPolicy>>,
    /// Allowlist
    allow_from: Arc<RwLock<Vec<String>>>,
    /// Message sender
    message_tx: Arc<RwLock<Option<mpsc::UnboundedSender<IncomingMessage>>>>,
}

impl std::fmt::Debug for ImessageChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImessageChannel")
            .field("config", &self.config)
            .field("running", &self.running)
            .finish()
    }
}

impl ImessageChannel {
    /// Create a new iMessage channel
    pub fn new(config: ImessageConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            config,
            http_client,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            message_map: Arc::new(RwLock::new(HashMap::new())),
            pairing_store: Arc::new(RwLock::new(None)),
            dm_policy: Arc::new(RwLock::new(DmPolicy::Open)),
            allow_from: Arc::new(RwLock::new(Vec::new())),
            message_tx: Arc::new(RwLock::new(None)),
        }
    }

    /// Set pairing store
    pub async fn set_pairing_store(&self, store: Arc<PairingStore>) {
        let mut s = self.pairing_store.write().await;
        *s = Some(store);
    }

    /// Set DM policy
    pub async fn set_dm_policy(&self, policy: DmPolicy) {
        let mut p = self.dm_policy.write().await;
        *p = policy;
    }

    /// Set allowlist
    pub async fn set_allow_from(&self, handles: Vec<String>) {
        let mut a = self.allow_from.write().await;
        *a = handles;
    }

    /// Set message sender
    pub async fn set_message_sender(&self, sender: mpsc::UnboundedSender<IncomingMessage>) {
        let mut tx = self.message_tx.write().await;
        *tx = Some(sender);
    }

    /// Check if an iMessage user is authorized
    pub async fn check_access(
        &self,
        user_id: &str,
        user_name: Option<&str>,
    ) -> (bool, Option<String>) {
        let policy = *self.dm_policy.read().await;
        match policy {
            DmPolicy::Open => (true, None),
            DmPolicy::Allowlist => {
                let allow_from = self.allow_from.read().await;
                if allow_from.contains(&user_id.to_string()) {
                    (true, None)
                } else {
                    (false, Some("You are not authorized to use this bot.".to_string()))
                }
            }
            DmPolicy::Pairing => {
                let store_guard = self.pairing_store.read().await;
                if let Some(store) = store_guard.as_ref() {
                    match store.request_access("imessage", user_id, user_name).await {
                        Ok(RequestAccessResult::AlreadyAuthorized) => (true, None),
                        Ok(RequestAccessResult::AlreadyPending { code, .. }) => (
                            false,
                            Some(format!(
                                "Your access request is pending admin approval. Pairing code: `{}`",
                                code
                            )),
                        ),
                        Ok(RequestAccessResult::NewRequest { code }) => (
                            false,
                            Some(format!(
                                "Access requested. An admin will approve your request.\nPairing \
                                 code: `{}`",
                                code
                            )),
                        ),
                        Ok(RequestAccessResult::RateLimited { .. }) => {
                            (false, Some("Too many requests. Please try again later.".to_string()))
                        }
                        Err(_) => {
                            (false, Some("An error occurred processing your request.".to_string()))
                        }
                    }
                } else {
                    (false, Some("Access control is not configured.".to_string()))
                }
            }
        }
    }

    /// Build request with optional auth
    fn build_request(&self, method: reqwest::Method, endpoint: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.config.server_url, endpoint);
        let mut req = self.http_client.request(method, &url);

        if let Some(ref password) = self.config.api_password {
            req = req.header("Authorization", format!("Bearer {}", password));
        }

        req
    }

    /// Send an iMessage via BlueBubbles
    async fn send_imessage(&self, recipient: &str, content: &str) -> crate::Result<String> {
        // Determine if recipient is a chat GUID or individual address
        let (chat_guid, address) =
            if recipient.starts_with("iMessage;+") || recipient.starts_with("SMS;+") {
                (Some(recipient.to_string()), None)
            } else {
                (None, Some(recipient.to_string()))
            };

        let payload = BbSendRequest {
            chat_guid,
            address,
            message: content.to_string(),
            method: Some("private-api".to_string()),
        };

        let response = self
            .build_request(reqwest::Method::POST, "/api/v1/message")
            .json(&payload)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: format!("BlueBubbles send request failed: {}", e),
                cause: Some(Box::new(e)),
            })?;

        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap_or_default();

        if !status.is_success() {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!(
                    "BlueBubbles API error ({}): {}",
                    status,
                    body.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown")
                ),
                cause: None,
            });
        }

        // Extract message GUID from response
        let guid = body
            .get("data")
            .and_then(|d| d.get("guid"))
            .and_then(|g| g.as_str())
            .unwrap_or("")
            .to_string();

        Ok(guid)
    }

    /// Format content for iMessage
    fn format_for_imessage(text: &str) -> String {
        // iMessage supports basic formatting through Unicode
        let mut result = text.to_string();

        // Strip HTML tags
        result = RE_HTML_TAG.replace_all(&result, "").to_string();

        result
    }
}

#[async_trait]
impl Channel for ImessageChannel {
    fn name(&self) -> &str {
        "imessage"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            chat_types: vec![ChatType::Direct, ChatType::Group],
            supports_formatting: false, // iMessage uses rich format but through Apple-specific APIs
            supports_attachments: true,
            supports_images: true,
            supports_threads: true, // iMessage supports reply threads
            supports_typing: true,
            supports_buttons: false,
            supports_commands: false,
            supports_reactions: true, // Tapbacks
            supports_edit: true,      // iOS 16+ supports editing
            supports_unsend: true,    // iOS 16+ supports unsend
            supports_effects: false,
        }
    }

    async fn start(&self) -> crate::Result<()> {
        info!("Starting iMessage channel via BlueBubbles");

        // Verify BlueBubbles server is reachable
        match self
            .build_request(reqwest::Method::GET, "/api/v1/server/info")
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_success() {
                    info!("BlueBubbles server connected at {}", self.config.server_url);
                } else {
                    warn!("BlueBubbles server returned status {}", response.status());
                }
            }
            Err(e) => {
                warn!(
                    "Could not connect to BlueBubbles server at {}: {}",
                    self.config.server_url, e
                );
                warn!("Make sure BlueBubbles is running on your Mac.");
            }
        }

        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        info!("iMessage channel started");
        info!("Note: WebSocket configuration recommended for receiving messages");

        // Keep running until stopped
        while self.running.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(&self) -> crate::Result<()> {
        info!("Stopping iMessage channel...");
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id> {
        let recipient = &message.conversation_id.0;

        // Format content
        let content = match &message.formatted_content {
            Some(FormattedContent::Markdown(md)) => Self::format_for_imessage(md),
            Some(FormattedContent::Html(html)) => Self::format_for_imessage(html),
            _ => message.content,
        };

        let guid = self.send_imessage(recipient, &content).await?;

        let msg_id = Id::new();
        if !guid.is_empty() {
            let mut map = self.message_map.write().await;
            map.insert(msg_id.to_string(), guid.clone());
        }

        debug!("iMessage sent to {} with GUID {}", recipient, guid);
        Ok(msg_id)
    }

    async fn send_typing(&self, _conversation_id: &ConversationId) -> crate::Result<()> {
        // BlueBubbles supports typing indicators via WebSocket
        // For REST-only mode, this is a no-op
        Ok(())
    }

    async fn edit_message(&self, _message_id: Id, _new_content: String) -> crate::Result<()> {
        // BlueBubbles API supports editing on newer macOS versions
        // Implementation would require PATCH /api/v1/message/:guid
        Err(crate::error::SyscityError::Internal(
            "iMessage editing not yet implemented".to_string(),
        ))
    }

    async fn delete_message(&self, message_id: Id) -> crate::Result<()> {
        let msg_key = message_id.to_string();
        let guid = {
            let map = self.message_map.read().await;
            map.get(&msg_key)
                .cloned()
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!("iMessage {} not found", msg_key),
                })?
        };

        let response = self
            .build_request(reqwest::Method::DELETE, &format!("/api/v1/message/{}", guid))
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: format!("BlueBubbles delete request failed: {}", e),
                cause: Some(Box::new(e)),
            })?;

        if !response.status().is_success() {
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("BlueBubbles delete failed: {}", response.status()),
                cause: None,
            });
        }

        let mut map = self.message_map.write().await;
        map.remove(&msg_key);

        Ok(())
    }

    async fn health_check(&self) -> crate::Result<bool> {
        if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(false);
        }

        match self
            .build_request(reqwest::Method::GET, "/api/v1/server/info")
            .send()
            .await
        {
            Ok(response) => Ok(response.status().is_success()),
            Err(e) => {
                warn!("iMessage health check failed: {}", e);
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_imessage_config() {
        let config = ImessageConfig::new()
            .with_server_url("http://192.168.1.100:3000")
            .with_password("secret123")
            .allow_handles(vec!["user@example.com".to_string()]);

        assert_eq!(config.server_url, "http://192.168.1.100:3000");
        assert_eq!(config.api_password, Some("secret123".to_string()));
        assert_eq!(config.allowed_handles.len(), 1);
    }

    #[test]
    fn test_imessage_config_default() {
        let config = ImessageConfig::default();
        assert_eq!(config.server_url, DEFAULT_BLUEBUBBLES_URL);
        assert!(config.api_password.is_none());
    }

    #[test]
    fn test_format_for_imessage() {
        let input = "Hello <b>world</b>!";
        let output = ImessageChannel::format_for_imessage(input);
        assert!(!output.contains("<b>"));
        assert!(output.contains("Hello world!"));
    }

    #[test]
    fn test_imessage_capabilities() {
        let config = ImessageConfig::new();
        let channel = ImessageChannel::new(config);
        let caps = channel.capabilities();
        assert!(caps.supports_attachments);
        assert!(caps.supports_images);
        assert!(caps.supports_reactions);
        assert!(caps.supports_edit);
        assert!(caps.supports_unsend);
        assert!(caps.supports_threads);
    }
}
