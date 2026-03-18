//! QQ Channel Implementation
//!
//! This module implements the Channel trait for QQ using the Tencent QQ Bot API.
//! Requires: Tencent developer account and bot registration.

use crate::channels::{
    Channel, ChannelCapabilities, ConversationId, FormattedContent, OutgoingMessage,
};
use crate::core::models::Id;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// QQ Bot API base URL
const QQ_API_BASE: &str = "https://api.sgroup.qq.com";
const QQ_SANDBOX_BASE: &str = "https://sandbox.api.sgroup.qq.com";

/// QQ channel configuration
#[derive(Debug, Clone)]
pub struct QqConfig {
    /// App ID from QQ Open Platform
    pub app_id: String,
    /// App Secret from QQ Open Platform
    pub app_secret: String,
    /// Bot QQ number
    pub bot_qq: String,
    /// Access token
    pub access_token: String,
    /// Optional allowed QQ numbers (empty = allow all)
    pub allowed_qqs: Vec<String>,
    /// Use sandbox environment
    pub use_sandbox: bool,
    /// Intents (bitmap)
    pub intents: u32,
}

impl QqConfig {
    /// Create new config with app credentials
    pub fn new(
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
        bot_qq: impl Into<String>,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            app_secret: app_secret.into(),
            bot_qq: bot_qq.into(),
            access_token: String::new(),
            allowed_qqs: Vec::new(),
            use_sandbox: false,
            intents: 0, // Will be set properly
        }
    }

    /// Set access token
    pub fn with_access_token(mut self, token: impl Into<String>) -> Self {
        self.access_token = token.into();
        self
    }

    /// Set allowed QQ numbers
    pub fn allow_qqs(mut self, qqs: Vec<String>) -> Self {
        self.allowed_qqs = qqs;
        self
    }

    /// Use sandbox environment
    pub fn with_sandbox(mut self, use_sandbox: bool) -> Self {
        self.use_sandbox = use_sandbox;
        self
    }

    /// Set intents
    pub fn with_intents(mut self, intents: u32) -> Self {
        self.intents = intents;
        self
    }

    /// Get base URL
    fn base_url(&self) -> &str {
        if self.use_sandbox {
            QQ_SANDBOX_BASE
        } else {
            QQ_API_BASE
        }
    }
}

/// QQ API response wrapper
#[derive(Debug, Deserialize)]
struct QqResponse<T> {
    code: i32,
    message: String,
    data: Option<T>,
}

/// QQ message request
#[derive(Debug, Serialize)]
struct QqMessageRequest {
    #[serde(rename = "guild_id", skip_serializing_if = "Option::is_none")]
    guild_id: Option<String>,
    #[serde(rename = "channel_id", skip_serializing_if = "Option::is_none")]
    channel_id: Option<String>,
    content: String,
    #[serde(rename = "msg_id", skip_serializing_if = "Option::is_none")]
    msg_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    markdown: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keyboard: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ark: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<String>,
}

/// QQ access token response
#[derive(Debug, Deserialize)]
struct QqTokenResponse {
    #[serde(rename = "access_token")]
    access_token: String,
    #[serde(rename = "expires_in")]
    #[allow(dead_code)]
    expires_in: i64,
}

/// QQ channel implementation
pub struct QqChannel {
    config: QqConfig,
    http_client: reqwest::Client,
    running: Arc<std::sync::atomic::AtomicBool>,
    /// Track message IDs
    message_map: Arc<RwLock<HashMap<String, String>>>,
    /// Current access token (may be refreshed)
    current_token: Arc<RwLock<String>>,
}

impl std::fmt::Debug for QqChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QqChannel")
            .field("config", &self.config)
            .field("running", &self.running)
            .finish()
    }
}

impl QqChannel {
    /// Create a new QQ channel
    pub fn new(config: QqConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        let initial_token = config.access_token.clone();

        Self {
            config,
            http_client,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            message_map: Arc::new(RwLock::new(HashMap::new())),
            current_token: Arc::new(RwLock::new(initial_token)),
        }
    }

    /// Check if QQ number is allowed
    fn is_qq_allowed(&self, qq: &str) -> bool {
        if self.config.allowed_qqs.is_empty() {
            return true;
        }
        self.config.allowed_qqs.iter().any(|q| q == qq)
    }

    /// Get access token (refresh if needed)
    async fn get_access_token(&self) -> crate::Result<String> {
        let token = self.current_token.read().await.clone();
        if !token.is_empty() {
            return Ok(token);
        }

        // Need to get token using app credentials
        self.refresh_token().await
    }

    /// Refresh access token
    async fn refresh_token(&self) -> crate::Result<String> {
        let url = format!("{}/app/getAppAccessToken", self.config.base_url());

        let params = serde_json::json!({
            "appId": self.config.app_id,
            "clientSecret": self.config.app_secret,
        });

        let response = self
            .http_client
            .post(&url)
            .json(&params)
            .send()
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: format!("Failed to get QQ access token: {}", e),
                cause: Some(Box::new(e)),
            })?;

        let token_resp: QqTokenResponse =
            response
                .json()
                .await
                .map_err(|e| crate::error::MantaError::ExternalService {
                    source: format!("Failed to parse QQ token response: {}", e),
                    cause: Some(Box::new(e)),
                })?;

        let mut token = self.current_token.write().await;
        *token = token_resp.access_token.clone();

        Ok(token_resp.access_token)
    }

    /// Make authenticated API request
    async fn api_request<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        payload: Option<serde_json::Value>,
    ) -> crate::Result<T> {
        let url = format!("{}{}", self.config.base_url(), endpoint);
        let token = self.get_access_token().await?;

        let mut request = self
            .http_client
            .request(method, &url)
            .header("Authorization", format!("QQBot {}", token))
            .header("X-Union-Appid", &self.config.app_id);

        if let Some(payload) = payload {
            request = request.json(&payload);
        }

        let response =
            request
                .send()
                .await
                .map_err(|e| crate::error::MantaError::ExternalService {
                    source: format!("QQ API request failed: {}", e),
                    cause: Some(Box::new(e)),
                })?;

        let result: T =
            response
                .json()
                .await
                .map_err(|e| crate::error::MantaError::ExternalService {
                    source: format!("Failed to parse QQ response: {}", e),
                    cause: Some(Box::new(e)),
                })?;

        Ok(result)
    }

    /// Send message to channel (guild) or user (direct message)
    async fn send_message(&self, req: QqMessageRequest) -> crate::Result<String> {
        let endpoint = if req.guild_id.is_some() {
            "/channels/{channel_id}/messages"
        } else {
            "/dms/{guild_id}/messages"
        };

        let channel_id = req
            .channel_id
            .as_ref()
            .or(req.guild_id.as_ref())
            .ok_or_else(|| {
                crate::error::MantaError::Validation("Channel ID or Guild ID required".to_string())
            })?;

        let endpoint = endpoint.replace("{channel_id}", channel_id);

        let response: QqResponse<serde_json::Value> = self
            .api_request(
                reqwest::Method::POST,
                &endpoint,
                Some(serde_json::to_value(&req).unwrap()),
            )
            .await?;

        if response.code != 0 {
            return Err(crate::error::MantaError::ExternalService {
                source: format!("QQ API error {}: {}", response.code, response.message),
                cause: None,
            });
        }

        // Extract message ID from response
        let msg_id = response
            .data
            .as_ref()
            .and_then(|d| d.get("id").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();

        Ok(msg_id)
    }

    /// Format content for QQ
    fn format_for_qq(text: &str) -> String {
        // QQ uses standard markdown mostly
        let mut result = text.to_string();

        // Bold: **text** -> **text* (QQ uses single asterisk variants or plain text)
        // QQ bot API supports markdown with limited formatting

        // Convert HTML-like formatting to markdown
        result = result.replace("<b>", "**").replace("</b>", "**");
        result = result.replace("<i>", "*").replace("</i>", "*");
        result = result.replace("<code>", "`").replace("</code>", "`");
        result = result.replace("<pre>", "```\n").replace("</pre>", "\n```");

        result
    }
}

#[async_trait]
impl Channel for QqChannel {
    fn name(&self) -> &str {
        "qq"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            chat_types: vec![
                crate::channels::ChatType::Direct,
                crate::channels::ChatType::Group,
                crate::channels::ChatType::Channel,
            ],
            supports_formatting: true,
            supports_attachments: true,
            supports_images: true,
            supports_threads: true,
            supports_typing: false,
            supports_buttons: true,
            supports_commands: true,
            supports_reactions: true,
            supports_edit: true,
            supports_unsend: false,
            supports_effects: false,
        }
    }

    async fn start(&self) -> crate::Result<()> {
        info!("Starting QQ channel...");

        // Test connection by getting access token
        if self.config.access_token.is_empty() {
            match self.refresh_token().await {
                Ok(token) => {
                    info!("QQ access token obtained successfully");
                    debug!("Token length: {}", token.len());
                }
                Err(e) => {
                    warn!("Failed to get QQ access token: {}", e);
                    // Continue anyway, might be configured with static token
                }
            }
        }

        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        info!("QQ channel started");
        info!("Note: WebSocket/Webhook configuration required for receiving messages");

        // Keep running until stopped
        while self.running.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(&self) -> crate::Result<()> {
        info!("Stopping QQ channel...");
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id> {
        let recipient = &message.conversation_id.0;

        // Check if allowed
        if !self.is_qq_allowed(recipient) {
            return Err(crate::error::MantaError::Validation(format!(
                "QQ {} is not in allow list",
                recipient
            )));
        }

        // Format content
        let content = match &message.formatted_content {
            Some(FormattedContent::Markdown(md)) => Self::format_for_qq(md),
            Some(FormattedContent::Html(html)) => Self::format_for_qq(html),
            _ => message.content,
        };

        // Determine if it's a DM or guild message
        // QQ conversation IDs starting with "dm:" are direct messages
        let is_dm = recipient.starts_with("dm:");

        let req = if is_dm {
            QqMessageRequest {
                guild_id: Some(recipient.trim_start_matches("dm:").to_string()),
                channel_id: None,
                content,
                msg_id: None,
                markdown: None,
                keyboard: None,
                ark: None,
                image: None,
            }
        } else {
            QqMessageRequest {
                guild_id: None,
                channel_id: Some(recipient.to_string()),
                content,
                msg_id: None,
                markdown: None,
                keyboard: None,
                ark: None,
                image: None,
            }
        };

        let msg_id = self.send_message(req).await?;

        // Track message
        let mut map = self.message_map.write().await;
        map.insert(msg_id.clone(), recipient.to_string());

        debug!("QQ message sent to {} with ID {}", recipient, msg_id);
        Ok(Id::new())
    }

    async fn send_typing(&self, _conversation_id: &ConversationId) -> crate::Result<()> {
        // QQ doesn't have a typing indicator API
        Ok(())
    }

    async fn edit_message(&self, message_id: Id, new_content: String) -> crate::Result<()> {
        let msg_id_str = message_id.to_string();

        // Look up where this message was sent
        let channel_id = {
            let map = self.message_map.read().await;
            map.get(&msg_id_str)
                .cloned()
                .ok_or_else(|| crate::error::MantaError::NotFound {
                    resource: format!("Message {} not found", msg_id_str),
                })?
        };

        // QQ API: PATCH /channels/{channel_id}/messages/{message_id}
        let endpoint = format!("/channels/{}/messages/{}", channel_id, msg_id_str);

        let payload = serde_json::json!({
            "content": Self::format_for_qq(&new_content),
        });

        let response: QqResponse<serde_json::Value> = self
            .api_request(reqwest::Method::PATCH, &endpoint, Some(payload))
            .await?;

        if response.code != 0 {
            return Err(crate::error::MantaError::ExternalService {
                source: format!("QQ edit failed: {}", response.message),
                cause: None,
            });
        }

        Ok(())
    }

    async fn delete_message(&self, message_id: Id) -> crate::Result<()> {
        let msg_id_str = message_id.to_string();

        // Look up where this message was sent
        let channel_id = {
            let map = self.message_map.read().await;
            map.get(&msg_id_str)
                .cloned()
                .ok_or_else(|| crate::error::MantaError::NotFound {
                    resource: format!("Message {} not found", msg_id_str),
                })?
        };

        // QQ API: DELETE /channels/{channel_id}/messages/{message_id}
        let endpoint = format!("/channels/{}/messages/{}", channel_id, msg_id_str);

        let response: QqResponse<serde_json::Value> = self
            .api_request(reqwest::Method::DELETE, &endpoint, None)
            .await?;

        if response.code != 0 {
            return Err(crate::error::MantaError::ExternalService {
                source: format!("QQ delete failed: {}", response.message),
                cause: None,
            });
        }

        // Remove from tracking
        let mut map = self.message_map.write().await;
        map.remove(&msg_id_str);

        Ok(())
    }

    async fn health_check(&self) -> crate::Result<bool> {
        if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(false);
        }

        // Try to get access token
        match self.get_access_token().await {
            Ok(_) => Ok(true),
            Err(e) => {
                warn!("QQ health check failed: {}", e);
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qq_config() {
        let config = QqConfig::new("123456", "secret", "1234567890")
            .with_access_token("token123")
            .with_sandbox(true);

        assert_eq!(config.app_id, "123456");
        assert_eq!(config.app_secret, "secret");
        assert_eq!(config.bot_qq, "1234567890");
        assert_eq!(config.access_token, "token123");
        assert!(config.use_sandbox);
    }

    #[test]
    fn test_format_for_qq() {
        let input = "<b>bold</b> and <i>italic</i>";
        let output = QqChannel::format_for_qq(input);
        assert!(output.contains("**bold**"));
        assert!(output.contains("*italic*"));
    }

    #[test]
    fn test_base_url() {
        let config_sandbox = QqConfig::new("1", "s", "qq").with_sandbox(true);
        assert_eq!(config_sandbox.base_url(), QQ_SANDBOX_BASE);

        let config_prod = QqConfig::new("1", "s", "qq").with_sandbox(false);
        assert_eq!(config_prod.base_url(), QQ_API_BASE);
    }
}
