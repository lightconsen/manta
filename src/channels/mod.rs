//! Channel abstractions for Manta
//!
//! Channels are communication interfaces through which users interact
//! with the AI assistant (CLI, Telegram, Discord, Slack, etc.).

use crate::core::models::Id;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

pub mod formatter;

/// Channel types supported by Manta
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    /// WhatsApp via Meta Business API
    Whatsapp,
    /// Telegram Bot API
    Telegram,
    /// Feishu/Lark Open API
    Feishu,
    /// QQ via go-cqhttp
    Qq,
    /// Discord Gateway
    Discord,
    /// Slack Socket Mode
    Slack,
    /// Custom WebSocket endpoint
    Websocket,
    /// Web terminal (built-in)
    WebTerminal,
}

#[cfg(feature = "telegram")]
pub mod telegram;

#[cfg(feature = "discord")]
pub mod discord;

#[cfg(feature = "slack")]
pub mod slack;

#[cfg(feature = "whatsapp")]
pub mod whatsapp;

#[cfg(feature = "qq")]
pub mod qq;

#[cfg(feature = "lark")]
pub mod lark;

#[cfg(feature = "plugins")]
pub mod plugin_host;

pub use formatter::{
    DiscordFormatter, MessageFormatter, PlainTextFormatter, SlackFormatter, TelegramHtmlFormatter,
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
    /// Supports message reactions (emoji reactions)
    pub supports_reactions: bool,
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
            supports_reactions: false,
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
        Self { channels: HashMap::new() }
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

/// Input validation and sanitization for messages
pub mod validation {

    /// Default maximum message length (10,000 characters)
    pub const DEFAULT_MAX_MESSAGE_LENGTH: usize = 10_000;

    /// Minimum message length (non-empty)
    pub const MIN_MESSAGE_LENGTH: usize = 1;

    /// Validation error for incoming messages
    #[derive(Debug, Clone, PartialEq)]
    pub enum ValidationError {
        /// Message is too long
        TooLong { max: usize, actual: usize },
        /// Message is too short (empty)
        TooShort { min: usize, actual: usize },
        /// Contains potentially dangerous content
        SuspiciousContent(String),
        /// Contains control characters
        ControlCharacters(String),
    }

    impl std::fmt::Display for ValidationError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::TooLong { max, actual } => {
                    write!(f, "Message too long: {} characters (max {})", actual, max)
                }
                Self::TooShort { min, actual } => {
                    write!(f, "Message too short: {} characters (min {})", actual, min)
                }
                Self::SuspiciousContent(reason) => {
                    write!(f, "Suspicious content detected: {}", reason)
                }
                Self::ControlCharacters(chars) => {
                    write!(f, "Control characters not allowed: {}", chars)
                }
            }
        }
    }

    impl std::error::Error for ValidationError {}

    /// Message validator with configurable limits
    #[derive(Debug, Clone)]
    pub struct MessageValidator {
        max_length: usize,
        min_length: usize,
        allow_control_chars: bool,
        sanitize_html: bool,
    }

    impl Default for MessageValidator {
        fn default() -> Self {
            Self {
                max_length: DEFAULT_MAX_MESSAGE_LENGTH,
                min_length: MIN_MESSAGE_LENGTH,
                allow_control_chars: false,
                sanitize_html: true,
            }
        }
    }

    impl MessageValidator {
        /// Create a new validator with default settings
        pub fn new() -> Self {
            Self::default()
        }

        /// Set maximum message length
        pub fn with_max_length(mut self, max: usize) -> Self {
            self.max_length = max;
            self
        }

        /// Set minimum message length
        pub fn with_min_length(mut self, min: usize) -> Self {
            self.min_length = min;
            self
        }

        /// Allow control characters
        pub fn allow_control_chars(mut self, allow: bool) -> Self {
            self.allow_control_chars = allow;
            self
        }

        /// Enable/disable HTML sanitization
        pub fn with_html_sanitization(mut self, sanitize: bool) -> Self {
            self.sanitize_html = sanitize;
            self
        }

        /// Validate a message, returning an error if invalid
        pub fn validate(&self, message: &str) -> Result<(), ValidationError> {
            let length = message.chars().count();

            // Check minimum length
            if length < self.min_length {
                return Err(ValidationError::TooShort {
                    min: self.min_length,
                    actual: length,
                });
            }

            // Check maximum length
            if length > self.max_length {
                return Err(ValidationError::TooLong {
                    max: self.max_length,
                    actual: length,
                });
            }

            // Check for control characters
            if !self.allow_control_chars {
                let control_chars: Vec<char> = message
                    .chars()
                    .filter(|c| c.is_control() && !c.is_whitespace())
                    .collect();
                if !control_chars.is_empty() {
                    return Err(ValidationError::ControlCharacters(control_chars.iter().collect()));
                }
            }

            // Check for null bytes
            if message.contains('\0') {
                return Err(ValidationError::SuspiciousContent(
                    "Null bytes not allowed".to_string(),
                ));
            }

            Ok(())
        }

        /// Sanitize a message, removing/replacing dangerous content
        pub fn sanitize(&self, message: &str) -> String {
            let mut sanitized = message.to_string();

            // Remove null bytes
            sanitized = sanitized.replace('\0', "");

            // Remove control characters (except whitespace)
            if !self.allow_control_chars {
                sanitized = sanitized
                    .chars()
                    .filter(|c| !c.is_control() || c.is_whitespace())
                    .collect();
            }

            // Trim leading/trailing whitespace
            sanitized = sanitized.trim().to_string();

            // Limit length if too long
            if sanitized.chars().count() > self.max_length {
                sanitized = sanitized.chars().take(self.max_length).collect();
            }

            sanitized
        }

        /// Validate and sanitize in one step
        pub fn validate_and_sanitize(&self, message: &str) -> Result<String, ValidationError> {
            let sanitized = self.sanitize(message);
            self.validate(&sanitized)?;
            Ok(sanitized)
        }
    }

    /// Quick validation function for simple use cases
    pub fn validate_message(message: &str) -> Result<(), ValidationError> {
        let validator = MessageValidator::new();
        validator.validate(message)
    }

    /// Quick sanitization function for simple use cases
    pub fn sanitize_message(message: &str) -> String {
        let validator = MessageValidator::new();
        validator.sanitize(message)
    }
}

// Re-export channel implementations
#[cfg(feature = "telegram")]
pub use telegram::{TelegramChannel, TelegramConfig};

#[cfg(feature = "discord")]
pub use discord::{DiscordChannel, DiscordConfig};

#[cfg(feature = "slack")]
pub use slack::{SlackChannel, SlackConfig};

#[cfg(feature = "whatsapp")]
pub use whatsapp::{WhatsappChannel, WhatsappConfig};

#[cfg(feature = "qq")]
pub use qq::{QqChannel, QqConfig};

#[cfg(feature = "lark")]
pub use lark::{LarkChannel, LarkConfig};

#[cfg(feature = "plugins")]
pub use plugin_host::{PluginChannel, PluginChannelRegistry, PluginManifest};

/// Extended channel registry that supports both native and WASM plugins
#[cfg(feature = "plugins")]
pub struct ExtendedChannelRegistry {
    /// Native channels
    native: ChannelRegistry,
    /// WASM plugin channels
    plugins: Option<PluginChannelRegistry>,
}

#[cfg(feature = "plugins")]
impl ExtendedChannelRegistry {
    /// Create a new extended registry
    pub fn new(message_tx: mpsc::UnboundedSender<IncomingMessage>) -> Self {
        let plugin_dir = crate::dirs::extensions_dir().join("channels");
        Self {
            native: ChannelRegistry::new(),
            plugins: Some(PluginChannelRegistry::new(plugin_dir, message_tx)),
        }
    }

    /// Register a native channel
    pub fn register_native(&mut self, channel: BoxedChannel) {
        self.native.register(channel);
    }

    /// Get a channel by name (checks native first, then plugins)
    pub async fn get(&self, name: &str) -> Option<BoxedChannel> {
        // Check native channels first
        if let Some(channel) = self.native.get(name) {
            // This is a bit awkward since we can't clone BoxedChannel
            // In practice, you'd need to wrap channels in Arc or use a different approach
            return None; // Placeholder - would need Arc<dyn Channel> in practice
        }

        // Check plugin channels
        if let Some(ref plugins) = self.plugins {
            if let Some(plugin) = plugins.get_plugin(name).await {
                return Some(Box::new(plugin));
            }
        }

        None
    }

    /// List all available channel names
    pub async fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.native.list().into_iter().map(|s| s.to_string()).collect();

        if let Some(ref plugins) = self.plugins {
            names.extend(plugins.list_loaded().await);
        }

        names
    }

    /// Load a WASM plugin
    pub async fn load_plugin(&self, name: &str, config: Option<serde_json::Value>) -> crate::Result<()> {
        if let Some(ref plugins) = self.plugins {
            plugins.load_plugin(name, config).await?;
        }
        Ok(())
    }

    /// Unload a WASM plugin
    pub async fn unload_plugin(&self, name: &str) -> crate::Result<()> {
        if let Some(ref plugins) = self.plugins {
            plugins.unload_plugin(name).await?;
        }
        Ok(())
    }

    /// Discover available WASM plugins
    pub async fn discover_plugins(&self) -> crate::Result<Vec<(String, std::path::PathBuf)>> {
        if let Some(ref plugins) = self.plugins {
            plugins.discover_plugins().await
        } else {
            Ok(Vec::new())
        }
    }

    /// Start all channels (native and plugins)
    pub async fn start_all(&self) -> Vec<crate::Result<()>> {
        let mut results = self.native.start_all().await;

        if let Some(ref plugins) = self.plugins {
            results.extend(plugins.start_all().await);
        }

        results
    }

    /// Stop all channels
    pub async fn stop_all(&self) -> Vec<crate::Result<()>> {
        let mut results = self.native.stop_all().await;

        if let Some(ref plugins) = self.plugins {
            results.extend(plugins.stop_all().await);
        }

        results
    }
}

#[cfg(feature = "plugins")]
impl Default for ExtendedChannelRegistry {
    fn default() -> Self {
        let (message_tx, _) = mpsc::unbounded_channel();
        Self::new(message_tx)
    }
}

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
        let attachment =
            Attachment::new("test.txt", "text/plain").with_data(b"Hello World".to_vec());
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
