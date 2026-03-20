//! Slack Channel Implementation
//!
//! This module implements the Channel trait for Slack using the Web API.

use crate::channels::{
    Channel, ChannelCapabilities, ConversationId, FormattedContent, IncomingMessage,
    OutgoingMessage,
};
use crate::core::models::Id;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Slack channel configuration
#[derive(Debug, Clone)]
pub struct SlackConfig {
    /// Bot token (xoxb-...)
    pub bot_token: String,
    /// App token for Socket Mode (xapp-...)
    pub app_token: Option<String>,
    /// Optional allowed user IDs (empty = allow all)
    pub allowed_user_ids: Vec<String>,
    /// Message handler channel
    pub message_tx: Option<mpsc::UnboundedSender<IncomingMessage>>,
    /// Bot user ID (filled after connection)
    pub bot_user_id: Option<String>,
}

impl SlackConfig {
    /// Create new config with bot token
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            app_token: None,
            allowed_user_ids: Vec::new(),
            message_tx: None,
            bot_user_id: None,
        }
    }

    /// Set app token for Socket Mode
    pub fn with_app_token(mut self, app_token: impl Into<String>) -> Self {
        self.app_token = Some(app_token.into());
        self
    }

    /// Set allowed user IDs
    pub fn allow_user_ids(mut self, user_ids: Vec<String>) -> Self {
        self.allowed_user_ids = user_ids;
        self
    }

    /// Set message handler
    pub fn with_message_handler(mut self, tx: mpsc::UnboundedSender<IncomingMessage>) -> Self {
        self.message_tx = Some(tx);
        self
    }
}

/// Slack channel implementation
pub struct SlackChannel {
    config: SlackConfig,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Maps our internal message ID -> (slack_channel_id, slack_ts) for edit/delete
    message_ids: std::sync::Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, (String, String)>>,
    >,
}

impl std::fmt::Debug for SlackChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlackChannel")
            .field("config", &self.config)
            .field("running", &self.running)
            .finish()
    }
}

impl SlackChannel {
    /// Create a new Slack channel
    pub fn new(config: SlackConfig) -> Self {
        Self {
            config,
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            message_ids: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    /// Check if user is allowed
    #[allow(dead_code)]
    fn is_user_allowed(&self, user_id: &str) -> bool {
        if self.config.allowed_user_ids.is_empty() {
            return true;
        }
        self.config.allowed_user_ids.contains(&user_id.to_string())
    }

    /// Convert markdown to Slack mrkdwn format
    fn markdown_to_mrkdwn(text: &str) -> String {
        let mut result = text.to_string();

        // Use placeholders to protect patterns during conversion
        let bold_placeholder = "<<<BOLD>>>";
        let italic_placeholder = "<<<ITALIC>>>";

        // Step 1: Protect bold patterns (**text** and __text__)
        result = regex::Regex::new(r"\*\*(.+?)\*\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, &caps[1], bold_placeholder)
            })
            .to_string();
        result = regex::Regex::new(r"__(.+?)__")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, &caps[1], bold_placeholder)
            })
            .to_string();

        // Step 2: Protect italic patterns (*text*)
        // These become <<<ITALIC>>>text<<<ITALIC>>>
        result = regex::Regex::new(r"\*(.+?)\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", italic_placeholder, &caps[1], italic_placeholder)
            })
            .to_string();

        // Step 3: Restore bold placeholders as *text* (Slack bold)
        result = result.replace(bold_placeholder, "*");

        // Step 4: Restore italic placeholders as _text_ (Slack italic)
        result = result.replace(italic_placeholder, "_");

        // Strikethrough: ~~text~~ -> ~text~
        result = regex::Regex::new(r"~~(.+?)~~")
            .unwrap()
            .replace_all(&result, "~$1~")
            .to_string();

        // Links: [text](url) -> <url|text>
        result = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)")
            .unwrap()
            .replace_all(&result, "<$2|$1>")
            .to_string();

        result
    }

    /// Strip markdown formatting for plain text fallback
    fn strip_markdown(text: &str) -> String {
        let mut result = text.to_string();

        // Protect patterns with placeholders, then strip the markers
        let bold_placeholder = "<<<BOLD>>>";
        let italic_placeholder = "<<<ITALIC>>>";

        // Protect bold patterns
        result = regex::Regex::new(r"\*\*(.+?)\*\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, &caps[1], bold_placeholder)
            })
            .to_string();
        result = regex::Regex::new(r"__(.+?)__")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, &caps[1], bold_placeholder)
            })
            .to_string();

        // Protect italic patterns
        result = regex::Regex::new(r"\*(.+?)\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", italic_placeholder, &caps[1], italic_placeholder)
            })
            .to_string();

        // Restore and strip bold placeholders (keep content only)
        result = result.replace(bold_placeholder, "");

        // Restore and strip italic placeholders (keep content only)
        result = result.replace(italic_placeholder, "");

        result = regex::Regex::new(r"_(.+?)_")
            .unwrap()
            .replace_all(&result, "$1")
            .to_string();
        result = regex::Regex::new(r"`([^`]+)`")
            .unwrap()
            .replace_all(&result, "$1")
            .to_string();
        result = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)")
            .unwrap()
            .replace_all(&result, "$1 ($2)")
            .to_string();

        result
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        "slack"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            chat_types: vec![
                crate::channels::ChatType::Direct,
                crate::channels::ChatType::Channel,
                crate::channels::ChatType::Thread,
            ],
            supports_formatting: true,
            supports_attachments: true,
            supports_images: true,
            supports_threads: true,
            supports_typing: false, // Slack doesn't have typing indicators in the same way
            supports_buttons: true,
            supports_commands: true,
            supports_reactions: true,
            supports_edit: true,
            supports_unsend: true,
            supports_effects: false,
        }
    }

    async fn start(&self) -> crate::Result<()> {
        #[cfg(feature = "slack")]
        {
            info!("Starting Slack channel");

            // Test the connection using reqwest
            let client = reqwest::Client::new();
            let resp = client
                .post("https://slack.com/api/auth.test")
                .header("Authorization", format!("Bearer {}", self.config.bot_token))
                .send()
                .await
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("Slack API request failed: {}", e))
                })?;

            let status = resp.status();
            if !status.is_success() {
                return Err(crate::error::MantaError::Internal(format!(
                    "Slack API returned error: {}",
                    status
                )));
            }

            self.running
                .store(true, std::sync::atomic::Ordering::SeqCst);
            info!("Slack channel started");
            Ok(())
        }

        #[cfg(not(feature = "slack"))]
        {
            Err(crate::error::MantaError::Internal("Slack feature not enabled".to_string()))
        }
    }

    async fn stop(&self) -> crate::Result<()> {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        info!("Slack channel stopped");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id> {
        #[cfg(feature = "slack")]
        {
            let channel_id = message.conversation_id.0.clone();

            // Format content
            let content = match &message.formatted_content {
                Some(FormattedContent::SlackMrkdwn(mrkdwn)) => mrkdwn.clone(),
                Some(FormattedContent::Markdown(md)) => Self::markdown_to_mrkdwn(md),
                Some(FormattedContent::Html(html)) => {
                    // Convert HTML to mrkdwn (simplified)
                    Self::strip_markdown(html)
                }
                _ => Self::markdown_to_mrkdwn(&message.content),
            };

            // Send message using reqwest
            let client = reqwest::Client::new();
            let resp = client
                .post("https://slack.com/api/chat.postMessage")
                .header("Authorization", format!("Bearer {}", self.config.bot_token))
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "channel": channel_id,
                    "text": content,
                }))
                .send()
                .await
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("Slack send failed: {}", e))
                })?;

            let resp_status = resp.status();
            let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();

            if resp_status.is_success() && resp_json["ok"].as_bool().unwrap_or(false) {
                let slack_ts =
                    resp_json["ts"].as_str().unwrap_or("").to_string();
                let slack_channel = resp_json["channel"]
                    .as_str()
                    .unwrap_or(&channel_id)
                    .to_string();
                let msg_id = Id::new();
                if !slack_ts.is_empty() {
                    let mut map = self.message_ids.write().await;
                    map.insert(msg_id.to_string(), (slack_channel, slack_ts));
                }
                debug!("Slack message sent successfully");
                Ok(msg_id)
            } else {
                Err(crate::error::MantaError::Internal(format!(
                    "Slack API error: {} — {}",
                    resp_status,
                    resp_json["error"].as_str().unwrap_or("unknown")
                )))
            }
        }

        #[cfg(not(feature = "slack"))]
        {
            let _ = message;
            Err(crate::error::MantaError::Internal("Slack feature not enabled".to_string()))
        }
    }

    async fn send_typing(&self, _conversation_id: &ConversationId) -> crate::Result<()> {
        // Slack doesn't have typing indicators in the same way as other platforms
        Ok(())
    }

    async fn edit_message(&self, message_id: Id, new_content: String) -> crate::Result<()> {
        #[cfg(feature = "slack")]
        {
            let msg_key = message_id.to_string();
            let (slack_channel, slack_ts) = {
                let map = self.message_ids.read().await;
                map.get(&msg_key).cloned().ok_or_else(|| {
                    crate::error::MantaError::NotFound {
                        resource: format!(
                            "Slack message {} not found in tracking (may have been sent before bot started)",
                            msg_key
                        ),
                    }
                })?
            };

            let client = reqwest::Client::new();
            let resp = client
                .post("https://slack.com/api/chat.update")
                .header("Authorization", format!("Bearer {}", self.config.bot_token))
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "channel": slack_channel,
                    "ts": slack_ts,
                    "text": new_content,
                }))
                .send()
                .await
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("Slack edit request failed: {}", e))
                })?;

            let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
            if !resp_json["ok"].as_bool().unwrap_or(false) {
                return Err(crate::error::MantaError::ExternalService {
                    source: format!(
                        "Slack chat.update failed: {}",
                        resp_json["error"].as_str().unwrap_or("unknown")
                    ),
                    cause: None,
                });
            }

            Ok(())
        }

        #[cfg(not(feature = "slack"))]
        {
            let _ = (message_id, new_content);
            Err(crate::error::MantaError::Internal("Slack feature not enabled".to_string()))
        }
    }

    async fn delete_message(&self, message_id: Id) -> crate::Result<()> {
        #[cfg(feature = "slack")]
        {
            let msg_key = message_id.to_string();
            let (slack_channel, slack_ts) = {
                let map = self.message_ids.read().await;
                map.get(&msg_key).cloned().ok_or_else(|| {
                    crate::error::MantaError::NotFound {
                        resource: format!(
                            "Slack message {} not found in tracking (may have been sent before bot started)",
                            msg_key
                        ),
                    }
                })?
            };

            let client = reqwest::Client::new();
            let resp = client
                .post("https://slack.com/api/chat.delete")
                .header("Authorization", format!("Bearer {}", self.config.bot_token))
                .header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "channel": slack_channel,
                    "ts": slack_ts,
                }))
                .send()
                .await
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!(
                        "Slack delete request failed: {}",
                        e
                    ))
                })?;

            let resp_json: serde_json::Value = resp.json().await.unwrap_or_default();
            if !resp_json["ok"].as_bool().unwrap_or(false) {
                return Err(crate::error::MantaError::ExternalService {
                    source: format!(
                        "Slack chat.delete failed: {}",
                        resp_json["error"].as_str().unwrap_or("unknown")
                    ),
                    cause: None,
                });
            }

            // Remove from tracking map
            let mut map = self.message_ids.write().await;
            map.remove(&msg_key);

            Ok(())
        }

        #[cfg(not(feature = "slack"))]
        {
            let _ = message_id;
            Err(crate::error::MantaError::Internal("Slack feature not enabled".to_string()))
        }
    }

    async fn health_check(&self) -> crate::Result<bool> {
        #[cfg(feature = "slack")]
        {
            // Simple check: verify we have a token and are running
            Ok(self.running.load(std::sync::atomic::Ordering::SeqCst)
                && !self.config.bot_token.is_empty())
        }

        #[cfg(not(feature = "slack"))]
        {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slack_config() {
        let config = SlackConfig::new("xoxb-test-token")
            .with_app_token("xapp-test-token")
            .allow_user_ids(vec!["U123".to_string()]);

        assert_eq!(config.bot_token, "xoxb-test-token");
        assert_eq!(config.app_token, Some("xapp-test-token".to_string()));
        assert_eq!(config.allowed_user_ids.len(), 1);
    }

    #[test]
    fn test_markdown_to_mrkdwn() {
        let md = "**bold** and *italic* and `code`";
        let mrkdwn = SlackChannel::markdown_to_mrkdwn(md);
        println!("Input: {}", md);
        println!("Output: {}", mrkdwn);
        assert!(mrkdwn.contains("*bold*"), "Expected *bold* in: {}", mrkdwn); // Slack bold is single asterisk
        assert!(mrkdwn.contains("_italic_"), "Expected _italic_ in: {}", mrkdwn); // Slack italic is underscore
        assert!(mrkdwn.contains("`code`"), "Expected `code` in: {}", mrkdwn); // Code stays the same
    }

    #[test]
    fn test_strip_markdown() {
        let md = "**bold** and [link](http://example.com)";
        let plain = SlackChannel::strip_markdown(md);
        assert!(plain.contains("bold"));
        assert!(!plain.contains("**"));
        assert!(plain.contains("link (http://example.com)"));
    }
}
