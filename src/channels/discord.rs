//! Discord Channel Implementation
//!
//! This module implements the Channel trait for Discord using serenity.

use crate::channels::{
    Attachment, Channel, ChannelCapabilities, ConversationId, DiscordEmbed, EmbedField,
    FormattedContent, IncomingMessage, MessageMetadata, MessageOptions, OutgoingMessage, UserId,
};
use crate::core::models::Id;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[cfg(feature = "discord")]
use serenity::{
    async_trait as serenity_async_trait,
    builder::{CreateEmbed, CreateMessage},
    client::{Context, EventHandler},
    model::{channel::Message, gateway::Ready, id::ChannelId},
    prelude::GatewayIntents,
    Client,
};

/// Discord channel configuration
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    /// Bot token
    pub token: String,
    /// Optional allowed user IDs (empty = allow all)
    pub allowed_user_ids: Vec<u64>,
    /// Message handler channel
    pub message_tx: Option<mpsc::UnboundedSender<IncomingMessage>>,
    /// Command prefix (e.g., "!")
    pub command_prefix: String,
}

impl DiscordConfig {
    /// Create new config with token
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            allowed_user_ids: Vec::new(),
            message_tx: None,
            command_prefix: "!".to_string(),
        }
    }

    /// Set allowed user IDs
    pub fn allow_user_ids(mut self, user_ids: Vec<u64>) -> Self {
        self.allowed_user_ids = user_ids;
        self
    }

    /// Set message handler
    pub fn with_message_handler(mut self, tx: mpsc::UnboundedSender<IncomingMessage>) -> Self {
        self.message_tx = Some(tx);
        self
    }

    /// Set command prefix
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.command_prefix = prefix.into();
        self
    }
}

/// Discord channel implementation
pub struct DiscordChannel {
    config: DiscordConfig,
    #[cfg(feature = "discord")]
    client: Option<Arc<tokio::sync::Mutex<Client>>>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl std::fmt::Debug for DiscordChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordChannel")
            .field("config", &self.config)
            .field("running", &self.running)
            .finish()
    }
}

impl DiscordChannel {
    /// Create a new Discord channel
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            config,
            #[cfg(feature = "discord")]
            client: None,
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Check if user is allowed
    fn is_user_allowed(&self, user_id: u64) -> bool {
        if self.config.allowed_user_ids.is_empty() {
            return true;
        }
        self.config.allowed_user_ids.contains(&user_id)
    }

    /// Convert markdown to Discord markdown (Discord uses standard markdown mostly)
    fn format_for_discord(text: &str) -> String {
        // Discord supports standard markdown well, but we need to handle some specifics
        let mut result = text.to_string();

        // Discord uses triple backticks for code blocks with language
        // Already supported in standard markdown

        // Spoiler tags: ||text|| (Discord specific)
        // We don't convert these as they're Discord-specific

        // Mentions: @user or @role - Discord handles these automatically

        result
    }

    /// Create a serenity embed from our DiscordEmbed
    #[cfg(feature = "discord")]
    fn create_serenity_embed(embed: &DiscordEmbed) -> CreateEmbed {
        let mut e = CreateEmbed::new();

        if let Some(title) = &embed.title {
            e = e.title(title);
        }

        if let Some(description) = &embed.description {
            e = e.description(description);
        }

        if let Some(color) = embed.color {
            e = e.color(color);
        }

        for field in &embed.fields {
            e = e.field(&field.name, &field.value, field.inline);
        }

        e
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
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
        }
    }

    async fn start(&self) -> crate::Result<()> {
        #[cfg(feature = "discord")]
        {
            info!("Starting Discord channel");

            let intents = GatewayIntents::GUILD_MESSAGES
                | GatewayIntents::DIRECT_MESSAGES
                | GatewayIntents::GUILDS
                | GatewayIntents::MESSAGE_CONTENT;

            let mut client = Client::builder(&self.config.token, intents)
                .event_handler(DiscordHandler {
                    config: self.config.clone(),
                })
                .await
                .map_err(|e| crate::error::MantaError::Internal(
                    format!("Discord client error: {}", e)
                ))?;

            self.running.store(true, std::sync::atomic::Ordering::SeqCst);

            tokio::spawn(async move {
                if let Err(why) = client.start().await {
                    error!("Discord client error: {:?}", why);
                }
            });

            info!("Discord channel started");
            Ok(())
        }

        #[cfg(not(feature = "discord"))]
        {
            Err(crate::error::MantaError::Internal(
                "Discord feature not enabled".to_string()
            ))
        }
    }

    async fn stop(&self) -> crate::Result<()> {
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
        info!("Discord channel stopped");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id> {
        #[cfg(feature = "discord")]
        {
            let channel_id: u64 = message
                .conversation_id
                .0
                .parse()
                .map_err(|_| crate::error::MantaError::Validation("Invalid channel ID".to_string()))?;

            // Note: This is a simplified implementation
            // In production, you'd need to store the client and use it here
            // For now, we just return a placeholder ID

            let _content = match &message.formatted_content {
                Some(FormattedContent::Markdown(md)) => Self::format_for_discord(md),
                Some(FormattedContent::DiscordEmbed(embed)) => {
                    // Would create embed here
                    format!("Embed: {:?}", embed)
                }
                _ => message.content,
            };

            // TODO: Actually send the message using the Discord client
            // This requires storing the client properly and accessing the HTTP client

            Ok(Id::new())
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = message;
            Err(crate::error::MantaError::Internal(
                "Discord feature not enabled".to_string()
            ))
        }
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> crate::Result<()> {
        #[cfg(feature = "discord")]
        {
            let _ = conversation_id;
            // Would trigger typing indicator in the channel
            // Requires access to the HTTP client
            Ok(())
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = conversation_id;
            Err(crate::error::MantaError::Internal(
                "Discord feature not enabled".to_string()
            ))
        }
    }

    async fn edit_message(&self, message_id: Id, new_content: String) -> crate::Result<()> {
        #[cfg(feature = "discord")]
        {
            let _ = (message_id, new_content);
            // Would edit the message using the Discord client
            Ok(())
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = (message_id, new_content);
            Err(crate::error::MantaError::Internal(
                "Discord feature not enabled".to_string()
            ))
        }
    }

    async fn delete_message(&self, message_id: Id) -> crate::Result<()> {
        #[cfg(feature = "discord")]
        {
            let _ = message_id;
            // Would delete the message using the Discord client
            Ok(())
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = message_id;
            Err(crate::error::MantaError::Internal(
                "Discord feature not enabled".to_string()
            ))
        }
    }

    async fn health_check(&self) -> crate::Result<bool> {
        #[cfg(feature = "discord")]
        {
            // Check if client is connected
            Ok(self.running.load(std::sync::atomic::Ordering::SeqCst))
        }

        #[cfg(not(feature = "discord"))]
        {
            Ok(false)
        }
    }
}

#[cfg(feature = "discord")]
struct DiscordHandler {
    config: DiscordConfig,
}

#[cfg(feature = "discord")]
#[serenity_async_trait]
impl EventHandler for DiscordHandler {
    async fn message(&self, ctx: Context, msg: Message) {
        // Ignore bot messages
        if msg.author.bot {
            return;
        }

        // Check if user is allowed
        if !self.config.allowed_user_ids.is_empty()
            && !self.config.allowed_user_ids.contains(&msg.author.id.get())
        {
            return;
        }

        debug!(
            "Received Discord message from {}: {}",
            msg.author.name, msg.content
        );

        // Handle DMs and mentions
        let is_dm = msg.guild_id.is_none();
        let is_mentioned = msg.mentions.iter().any(|u| u.bot);
        let has_prefix = msg.content.starts_with(&self.config.command_prefix);

        if is_dm || is_mentioned || has_prefix {
            let content = if has_prefix {
                msg.content[self.config.command_prefix.len()..].trim().to_string()
            } else {
                msg.content.clone()
            };

            let incoming = IncomingMessage::new(
                &msg.author.id.get().to_string(),
                &msg.channel_id.get().to_string(),
                content,
            )
            .with_metadata(
                MessageMetadata::new()
                    .with_extra("message_id", msg.id.get())
                    .with_extra("username", msg.author.name.clone())
                    .with_extra("is_dm", is_dm),
            );

            // Send to handler if configured
            if let Some(tx) = &self.config.message_tx {
                let _ = tx.send(incoming);
            }

            // Echo for now (replace with agent integration)
            let _ = msg
                .channel_id
                .say(&ctx.http, format!("Echo: {}", msg.content))
                .await;
        }
    }

    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("Discord bot connected as {}", ready.user.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discord_config() {
        let config = DiscordConfig::new("test_token")
            .allow_user_ids(vec![123456789])
            .with_prefix("!");

        assert_eq!(config.token, "test_token");
        assert_eq!(config.allowed_user_ids.len(), 1);
        assert_eq!(config.command_prefix, "!");
    }

    #[test]
    fn test_format_for_discord() {
        let md = "**bold** and *italic* and `code`";
        let formatted = DiscordChannel::format_for_discord(md);
        // Discord supports standard markdown, so it should remain similar
        assert!(formatted.contains("**bold**"));
        assert!(formatted.contains("*italic*"));
        assert!(formatted.contains("`code`"));
    }
}
