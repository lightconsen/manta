//! Channel abstractions for Manta
//!
//! Channels are communication interfaces through which users interact
//! with the AI assistant (CLI, Telegram, Discord, Slack, etc.).

use crate::core::models::Id;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod formatter;

#[cfg(feature = "telegram")]
pub mod telegram;

#[cfg(feature = "discord")]
pub mod discord;

#[cfg(feature = "slack")]
pub mod slack;

pub use formatter::{
    MessageFormatter, TelegramHtmlFormatter, DiscordFormatter,
    SlackFormatter, PlainTextFormatter
};

/// A user identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub String);

impl UserId {
    /// Create a new user ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A conversation/session identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(pub String);

impl ConversationId {
    /// Create a new conversation ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Generate a new unique conversation ID
    pub fn generate() -> Self {
        Self(crate::core::models::Id::new().to_string())
    }
}

impl std::fmt::Display for ConversationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Metadata about a message
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageMetadata {
    /// When the message was sent
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Channel-specific metadata
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl MessageMetadata {
    /// Create new metadata with current timestamp
    pub fn new() -> Self {
        Self {
            timestamp: chrono::Utc::now(),
            extra: HashMap::new(),
        }
    }

    /// Add extra metadata
    pub fn with_extra(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

/// An incoming message from a user
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    /// Unique message ID
    pub id: Id,
    /// The user who sent the message
    pub user_id: UserId,
    /// The conversation this message belongs to
    pub conversation_id: ConversationId,
    /// The content of the message
    pub content: String,
    /// Optional attachments (files, images, etc.)
    pub attachments: Vec<Attachment>,
    /// Message metadata
    pub metadata: MessageMetadata,
}

impl IncomingMessage {
    /// Create a new incoming message
    pub fn new(
        user_id: impl Into<String>,
        conversation_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: Id::new(),
            user_id: UserId::new(user_id),
            conversation_id: ConversationId::new(conversation_id),
            content: content.into(),
            attachments: Vec::new(),
            metadata: MessageMetadata::new(),
        }
    }

    /// Add an attachment
    pub fn with_attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    /// Set metadata
    pub fn with_metadata(mut self, metadata: MessageMetadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// An outgoing message to a user
#[derive(Debug, Clone)]
pub struct OutgoingMessage {
    /// The conversation to send to
    pub conversation_id: ConversationId,
    /// The content to send
    pub content: String,
    /// Optional formatted content (for rich formatting)
    pub formatted_content: Option<FormattedContent>,
    /// Optional attachments
    pub attachments: Vec<Attachment>,
    /// Whether this is a reply to a specific message
    pub reply_to: Option<Id>,
    /// Message options
    pub options: MessageOptions,
}

impl OutgoingMessage {
    /// Create a new outgoing message
    pub fn new(conversation_id: ConversationId, content: impl Into<String>) -> Self {
        Self {
            conversation_id,
            content: content.into(),
            formatted_content: None,
            attachments: Vec::new(),
            reply_to: None,
            options: MessageOptions::default(),
        }
    }

    /// Add formatted content
    pub fn with_formatted(mut self, content: FormattedContent) -> Self {
        self.formatted_content = Some(content);
        self
    }

    /// Add an attachment
    pub fn with_attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    /// Set reply-to message
    pub fn reply_to(mut self, message_id: Id) -> Self {
        self.reply_to = Some(message_id);
        self
    }

    /// Set message options
    pub fn with_options(mut self, options: MessageOptions) -> Self {
        self.options = options;
        self
    }
}

/// Formatted content for rich messages
#[derive(Debug, Clone)]
pub enum FormattedContent {
    /// Markdown formatted text
    Markdown(String),
    /// HTML formatted text
    Html(String),
    /// Slack mrkdwn format
    SlackMrkdwn(String),
    /// Discord embed
    DiscordEmbed(DiscordEmbed),
}

/// Discord embed structure
#[derive(Debug, Clone, Default)]
pub struct DiscordEmbed {
    pub title: Option<String>,
    pub description: Option<String>,
    pub color: Option<u32>,
    pub fields: Vec<EmbedField>,
}

/// A field in a Discord embed
#[derive(Debug, Clone)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

/// Message sending options
#[derive(Debug, Clone, Default)]
pub struct MessageOptions {
    /// Whether to send silently (no notification)
    pub silent: bool,
    /// Whether to expect a typing indicator first
    pub show_typing: bool,
    /// Custom metadata for the channel
    pub custom: HashMap<String, String>,
}

/// An attachment to a message
#[derive(Debug, Clone)]
pub struct Attachment {
    /// Unique ID for this attachment
    pub id: Id,
    /// The filename
    pub filename: String,
    /// MIME type
    pub content_type: String,
    /// File size in bytes
    pub size: usize,
    /// The actual data (optional, may be URL-based)
    pub data: Option<Vec<u8>>,
    /// URL to access the attachment (if hosted)
    pub url: Option<String>,
}

impl Attachment {
    /// Create a new attachment
    pub fn new(filename: impl Into<String>, content_type: impl Into<String>) -> Self {
        Self {
            id: Id::new(),
            filename: filename.into(),
            content_type: content_type.into(),
            size: 0,
            data: None,
            url: None,
        }
    }

    /// Set the attachment data
    pub fn with_data(mut self, data: Vec<u8>) -> Self {
        self.size = data.len();
        self.data = Some(data);
        self
    }

    /// Set the attachment URL
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

/// Channel capabilities
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelCapabilities {
    /// Supports formatted text (markdown, HTML, etc.)
    pub supports_formatting: bool,
    /// Supports file attachments
    pub supports_attachments: bool,
    /// Supports inline images
    pub supports_images: bool,
    /// Supports message threading/replies
    pub supports_threads: bool,
    /// Supports typing indicators
    pub supports_typing: bool,
    /// Supports reaction buttons
    pub supports_buttons: bool,
    /// Supports slash commands
    pub supports_commands: bool,
}

impl Default for ChannelCapabilities {
    fn default() -> Self {
        Self {
            supports_formatting: true,
            supports_attachments: true,
            supports_images: true,
            supports_threads: false,
            supports_typing: true,
            supports_buttons: false,
            supports_commands: false,
        }
    }
}

/// Trait for message channels
#[async_trait]
pub trait Channel: Send + Sync {
    /// Get the name of this channel
    fn name(&self) -> &str;

    /// Get the capabilities of this channel
    fn capabilities(&self) -> ChannelCapabilities;

    /// Start the channel (begin listening for messages)
    async fn start(&self) -> crate::Result<()>;

    /// Stop the channel
    async fn stop(&self) -> crate::Result<()>;

    /// Send a message
    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id>;

    /// Send a typing indicator
    async fn send_typing(&self, conversation_id: &ConversationId) -> crate::Result<()>;

    /// Edit a previously sent message
    async fn edit_message(&self, message_id: Id, new_content: String) -> crate::Result<()>;

    /// Delete a message
    async fn delete_message(&self, message_id: Id) -> crate::Result<()>;

    /// Check if the channel is healthy
    async fn health_check(&self) -> crate::Result<bool>;
}

/// A boxed channel for storage
pub type BoxedChannel = Box<dyn Channel>;

/// Registry of channels
#[derive(Default)]
pub struct ChannelRegistry {
    channels: HashMap<String, BoxedChannel>,
}

impl std::fmt::Debug for ChannelRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelRegistry")
            .field("channels", &self.channels.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ChannelRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }

    /// Register a channel
    pub fn register(&mut self, channel: BoxedChannel) {
        let name = channel.name().to_string();
        self.channels.insert(name, channel);
    }

    /// Get a channel by name
    pub fn get(&self, name: &str) -> Option<&dyn Channel> {
        self.channels.get(name).map(|c| c.as_ref())
    }

    /// List available channel names
    pub fn list(&self) -> Vec<&str> {
        self.channels.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a channel exists
    pub fn has(&self, name: &str) -> bool {
        self.channels.contains_key(name)
    }

    /// Start all channels
    pub async fn start_all(&self) -> Vec<crate::Result<()>> {
        let mut results = Vec::new();
        for channel in self.channels.values() {
            results.push(channel.start().await);
        }
        results
    }

    /// Stop all channels
    pub async fn stop_all(&self) -> Vec<crate::Result<()>> {
        let mut results = Vec::new();
        for channel in self.channels.values() {
            results.push(channel.stop().await);
        }
        results
    }
}

// Re-export channel implementations
#[cfg(feature = "telegram")]
pub use telegram::{TelegramChannel, TelegramConfig};

#[cfg(feature = "discord")]
pub use discord::{DiscordChannel, DiscordConfig};

#[cfg(feature = "slack")]
pub use slack::{SlackChannel, SlackConfig};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_id() {
        let id = UserId::new("user123");
        assert_eq!(id.0, "user123");
        assert_eq!(id.to_string(), "user123");
    }

    #[test]
    fn test_conversation_id() {
        let id = ConversationId::new("conv456");
        assert_eq!(id.0, "conv456");

        let generated = ConversationId::generate();
        assert!(!generated.0.is_empty());
    }

    #[test]
    fn test_incoming_message() {
        let msg = IncomingMessage::new("user1", "conv1", "Hello!");
        assert_eq!(msg.user_id.0, "user1");
        assert_eq!(msg.conversation_id.0, "conv1");
        assert_eq!(msg.content, "Hello!");
        assert!(msg.attachments.is_empty());
    }

    #[test]
    fn test_outgoing_message() {
        let conv_id = ConversationId::new("conv1");
        let msg = OutgoingMessage::new(conv_id, "Hi there!");
        assert_eq!(msg.content, "Hi there!");
        assert!(msg.formatted_content.is_none());

        let markdown = OutgoingMessage::new(ConversationId::new("conv1"), "Hello")
            .with_formatted(FormattedContent::Markdown("**Hello**".to_string()));
        assert!(matches!(markdown.formatted_content, Some(FormattedContent::Markdown(_))));
    }

    #[test]
    fn test_attachment() {
        let attachment = Attachment::new("test.txt", "text/plain")
            .with_data(b"Hello World".to_vec());
        assert_eq!(attachment.filename, "test.txt");
        assert_eq!(attachment.size, 11);
    }

    #[test]
    fn test_channel_capabilities() {
        let caps = ChannelCapabilities::default();
        assert!(caps.supports_formatting);
        assert!(caps.supports_attachments);
    }
}
