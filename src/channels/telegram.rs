//! Telegram Channel Implementation
//!
//! This module implements the Channel trait for Telegram using teloxide.

use crate::channels::{
    Attachment, Channel, ChannelCapabilities, ConversationId, FormattedContent,
    IncomingMessage, MessageMetadata, MessageOptions, OutgoingMessage, UserId,
};
use crate::core::models::Id;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[cfg(feature = "telegram")]
use teloxide::{
    dispatching::{dialogue::InMemStorage, Dispatcher, UpdateFilterExt},
    payloads::SendMessageSetters,
    prelude::*,
    types::{InputFile, Message, MessageId, ParseMode},
    Bot,
};

/// Telegram channel configuration
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    /// Bot token from @BotFather
    pub token: String,
    /// Optional allowed usernames (empty = allow all)
    pub allowed_usernames: Vec<String>,
    /// Message handler channel
    pub message_tx: Option<mpsc::UnboundedSender<IncomingMessage>>,
}

impl TelegramConfig {
    /// Create new config with token
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            allowed_usernames: Vec::new(),
            message_tx: None,
        }
    }

    /// Set allowed usernames
    pub fn allow_usernames(mut self, usernames: Vec<String>) -> Self {
        self.allowed_usernames = usernames;
        self
    }

    /// Set message handler
    pub fn with_message_handler(mut self, tx: mpsc::UnboundedSender<IncomingMessage>) -> Self {
        self.message_tx = Some(tx);
        self
    }
}

/// Telegram channel implementation
#[derive(Debug)]
pub struct TelegramChannel {
    config: TelegramConfig,
    bot: Option<Bot>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl TelegramChannel {
    /// Create a new Telegram channel
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            config,
            bot: None,
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Check if user is allowed
    fn is_user_allowed(&self, username: Option<&str>) -> bool {
        if self.config.allowed_usernames.is_empty() {
            return true;
        }
        username
            .map(|u| self.config.allowed_usernames.iter().any(|a| a.eq_ignore_ascii_case(u)))
            .unwrap_or(false)
    }

    /// Convert markdown to Telegram HTML
    fn markdown_to_telegram_html(text: &str) -> String {
        let mut result = text.to_string();

        // Escape HTML special characters first
        result = result
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");

        // Bold: **text** or __text__ -> <b>text</b>
        result = regex::Regex::new(r"\*\*(.+?)\*\*")
            .unwrap()
            .replace_all(&result, "<b>$1</b>")
            .to_string();
        result = regex::Regex::new(r"__(.+?)__")
            .unwrap()
            .replace_all(&result, "<b>$1</b>")
            .to_string();

        // Italic: *text* or _text_ -> <i>text</i>
        // Use placeholders to protect bold from being converted to italic
        let bold_placeholder = "\x00BOLD\x00";
        result = regex::Regex::new(r"\*\*(.+?)\*\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures| {
                format!("{}{}{}", bold_placeholder, caps.get(1).map(|m| m.as_str()).unwrap_or(""), bold_placeholder)
            })
            .to_string();

        result = regex::Regex::new(r"__(.+?)__")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures| {
                format!("{}{}{}", bold_placeholder, caps.get(1).map(|m| m.as_str()).unwrap_or(""), bold_placeholder)
            })
            .to_string();

        // Now process italic
        result = regex::Regex::new(r"\*([^*]+)\*")
            .unwrap()
            .replace_all(&result, "<i>$1</i>")
            .to_string();

        result = regex::Regex::new(r"_([^_]+)_")
            .unwrap()
            .replace_all(&result, "<i>$1</i>")
            .to_string();

        // Restore bold and apply formatting
        result = result.replace(bold_placeholder, "<b>$1</b>");

        // Code: `text` -> <code>text</code>
        result = regex::Regex::new(r"`([^`]+)`")
            .unwrap()
            .replace_all(&result, "<code>$1</code>")
            .to_string();

        // Code block: ```lang\ncode``` -> <pre><code class="language-lang">code</code></pre>
        result = regex::Regex::new(r"```(\w+)?\n(.*?)```")
            .unwrap()
            .replace_all(&result, "<pre><code>$2</code></pre>")
            .to_string();

        // Strikethrough: ~~text~~ -> <s>text</s>
        result = regex::Regex::new(r"~~(.+?)~~")
            .unwrap()
            .replace_all(&result, "<s>$1</s>")
            .to_string();

        // Links: [text](url) -> <a href="url">text</a>
        result = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)")
            .unwrap()
            .replace_all(&result, r#"<a href="$2">$1</a>"#)
            .to_string();

        result
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            supports_formatting: true,
            supports_attachments: true,
            supports_images: true,
            supports_threads: true,
            supports_typing: true,
            supports_buttons: true,
            supports_commands: true,
            supports_reactions: true,
        }
    }

    async fn start(&self) -> crate::Result<()> {
        #[cfg(feature = "telegram")]
        {
            info!("Starting Telegram channel");

            let bot = Bot::new(&self.config.token);
            let message_tx = self.config.message_tx.clone();
            let allowed_usernames = self.config.allowed_usernames.clone();
            let running = self.running.clone();

            running.store(true, std::sync::atomic::Ordering::SeqCst);

            tokio::spawn(async move {
                let handler = dptree::entry()
                    .branch(Update::filter_message().endpoint(handle_message));

                let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
                    .enable_ctrlc_handler()
                    .build();

                dispatcher.dispatch().await;
            });

            info!("Telegram channel started");
            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            Err(crate::error::MantaError::Internal(
                "Telegram feature not enabled".to_string()
            ))
        }
    }

    async fn stop(&self) -> crate::Result<()> {
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
        info!("Telegram channel stopped");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id> {
        #[cfg(feature = "telegram")]
        {
            let bot = self
                .bot
                .as_ref()
                .ok_or_else(|| crate::error::MantaError::Internal("Bot not initialized".to_string()))?;

            let chat_id: i64 = message
                .conversation_id
                .0
                .parse()
                .map_err(|_| crate::error::MantaError::Validation("Invalid chat ID".to_string()))?;

            // Format content
            let (text, parse_mode) = match message.formatted_content {
                Some(FormattedContent::Html(html)) => (html, Some(ParseMode::Html)),
                Some(FormattedContent::Markdown(md)) => {
                    (Self::markdown_to_telegram_html(&md), Some(ParseMode::Html))
                }
                _ => (message.content, None),
            };

            // Send message
            let mut req = bot.send_message(ChatId(chat_id), text);

            if let Some(mode) = parse_mode {
                req = req.parse_mode(mode);
            }

            if message.options.silent {
                req = req.disable_notification(true);
            }

            if let Some(_reply_id) = message.reply_to {
                // Note: reply_to contains our internal UUID, not the Telegram message ID
                // To properly implement replies, we'd need to map our UUID to Telegram's message ID
                // For now, skipping reply functionality
            }

            let _sent = req.await.map_err(|e| crate::error::MantaError::Internal(
                format!("Telegram send error: {}", e)
            ))?;

            Ok(Id::new())
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = message;
            Err(crate::error::MantaError::Internal(
                "Telegram feature not enabled".to_string()
            ))
        }
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> crate::Result<()> {
        #[cfg(feature = "telegram")]
        {
            let bot = self
                .bot
                .as_ref()
                .ok_or_else(|| crate::error::MantaError::Internal("Bot not initialized".to_string()))?;

            let chat_id: i64 = conversation_id
                .0
                .parse()
                .map_err(|_| crate::error::MantaError::Validation("Invalid chat ID".to_string()))?;

            bot.send_chat_action(ChatId(chat_id), teloxide::types::ChatAction::Typing)
                .await
                .map_err(|e| crate::error::MantaError::Internal(
                    format!("Telegram typing error: {}", e)
                ))?;

            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = conversation_id;
            Err(crate::error::MantaError::Internal(
                "Telegram feature not enabled".to_string()
            ))
        }
    }

    async fn edit_message(&self, message_id: Id, new_content: String) -> crate::Result<()> {
        #[cfg(feature = "telegram")]
        {
            let _ = (message_id, new_content);
            // Note: We need the chat_id to edit, which we don't have stored
            // This is a limitation - would need to track message_id -> chat_id mapping
            warn!("Edit message requires chat_id tracking, not fully implemented");
            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = (message_id, new_content);
            Err(crate::error::MantaError::Internal(
                "Telegram feature not enabled".to_string()
            ))
        }
    }

    async fn delete_message(&self, message_id: Id) -> crate::Result<()> {
        #[cfg(feature = "telegram")]
        {
            let _ = message_id;
            // Similar limitation as edit_message
            warn!("Delete message requires chat_id tracking, not fully implemented");
            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = message_id;
            Err(crate::error::MantaError::Internal(
                "Telegram feature not enabled".to_string()
            ))
        }
    }

    async fn health_check(&self) -> crate::Result<bool> {
        #[cfg(feature = "telegram")]
        {
            if let Some(bot) = &self.bot {
                match bot.get_me().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            } else {
                Ok(false)
            }
        }

        #[cfg(not(feature = "telegram"))]
        {
            Ok(false)
        }
    }
}

#[cfg(feature = "telegram")]
async fn handle_message(bot: Bot, msg: Message) -> ResponseResult<()> {
    if let Some(text) = msg.text() {
        debug!("Received message from {:?}: {}", msg.from(), text);

        // Create incoming message
        let user_id = msg
            .from()
            .map(|u| u.id.0.to_string())
            .unwrap_or_default();

        let chat_id = msg.chat.id.0.to_string();

        let incoming = IncomingMessage::new(&user_id, &chat_id, text)
            .with_metadata(
                MessageMetadata::new()
                    .with_extra("message_id", msg.id.0)
                    .with_extra("chat_type", format!("{:?}", msg.chat.kind)),
            );

        // TODO: Route to agent via message_tx
        // For now, just echo back
        let _ = incoming;
        bot.send_message(msg.chat.id, format!("Echo: {}", text)).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_to_telegram_html() {
        let md = "**bold** and *italic* and `code`";
        let html = TelegramChannel::markdown_to_telegram_html(md);
        assert!(html.contains("<b>bold</b>"));
        assert!(html.contains("<i>italic</i>"));
        assert!(html.contains("<code>code</code>"));
    }

    #[test]
    fn test_telegram_config() {
        let config = TelegramConfig::new("test_token").allow_usernames(vec!["test_user".to_string()]);
        assert_eq!(config.token, "test_token");
        assert_eq!(config.allowed_usernames.len(), 1);
    }
}
