//! Discord Channel Implementation
//!
//! This module implements the Channel trait for Discord using serenity.

use crate::channels::{
    Channel, ChannelCapabilities, ConversationId, DiscordEmbed, FormattedContent, IncomingMessage,
    MessageMetadata, OutgoingMessage,
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
    model::{
        channel::{Message, ReactionType},
        gateway::Ready,
        id::ChannelId,
    },
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
    /// Discord client (stored for future use)
    #[cfg(feature = "discord")]
    _client: Option<Arc<tokio::sync::Mutex<Client>>>,
    #[cfg(feature = "discord")]
    http: Option<Arc<serenity::http::Http>>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Message ID to Channel ID mapping for edit/delete operations
    #[cfg(feature = "discord")]
    message_channel_map: Arc<tokio::sync::RwLock<std::collections::HashMap<u64, u64>>>,
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
        #[cfg(feature = "discord")]
        let http = Some(Arc::new(serenity::http::Http::new(&config.token)));

        Self {
            config,
            #[cfg(feature = "discord")]
            _client: None,
            #[cfg(feature = "discord")]
            http,
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(feature = "discord")]
            message_channel_map: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    /// Track a message ID to channel ID mapping
    #[cfg(feature = "discord")]
    async fn track_message(&self, message_id: u64, channel_id: u64) {
        let mut map = self.message_channel_map.write().await;
        map.insert(message_id, channel_id);
    }

    /// Get channel ID for a message ID
    #[cfg(feature = "discord")]
    async fn get_message_channel(&self, message_id: u64) -> Option<u64> {
        let map = self.message_channel_map.read().await;
        map.get(&message_id).copied()
    }

    /// Check if user is allowed
    #[allow(dead_code)]
    fn is_user_allowed(&self, user_id: u64) -> bool {
        if self.config.allowed_user_ids.is_empty() {
            return true;
        }
        self.config.allowed_user_ids.contains(&user_id)
    }

    /// Convert markdown to Discord markdown (Discord uses standard markdown mostly)
    fn format_for_discord(text: &str) -> String {
        // Discord supports standard markdown well, but we need to handle some specifics
        let result = text.to_string();

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
            chat_types: vec![
                crate::channels::ChatType::Direct,
                crate::channels::ChatType::Group,
                crate::channels::ChatType::Channel,
                crate::channels::ChatType::Thread,
            ],
            supports_formatting: true,
            supports_attachments: true,
            supports_images: true,
            supports_threads: true,
            supports_typing: true,
            supports_buttons: true,
            supports_commands: true,
            supports_reactions: true,
            supports_edit: true,
            supports_unsend: true,
            supports_effects: false,
        }
    }

    async fn start(&self) -> crate::Result<()> {
        #[cfg(feature = "discord")]
        {
            info!("Starting Discord channel");

            let intents = GatewayIntents::GUILD_MESSAGES
                | GatewayIntents::DIRECT_MESSAGES
                | GatewayIntents::GUILDS
                | GatewayIntents::MESSAGE_CONTENT
                | GatewayIntents::GUILD_MESSAGE_REACTIONS
                | GatewayIntents::DIRECT_MESSAGE_REACTIONS;

            let mut client = Client::builder(&self.config.token, intents)
                .event_handler(DiscordHandler { config: self.config.clone() })
                .await
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("Discord client error: {}", e))
                })?;

            self.running
                .store(true, std::sync::atomic::Ordering::SeqCst);

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
            Err(crate::error::MantaError::Internal("Discord feature not enabled".to_string()))
        }
    }

    async fn stop(&self) -> crate::Result<()> {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        info!("Discord channel stopped");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id> {
        #[cfg(feature = "discord")]
        {
            let channel_id: u64 = message.conversation_id.0.parse().map_err(|_| {
                crate::error::MantaError::Validation("Invalid channel ID".to_string())
            })?;

            let http = self.http.as_ref().ok_or_else(|| {
                crate::error::MantaError::Internal("HTTP client not initialized".to_string())
            })?;

            let channel_id = ChannelId::new(channel_id);

            // Build the message content
            let content = match &message.formatted_content {
                Some(FormattedContent::Markdown(md)) => Self::format_for_discord(md),
                Some(FormattedContent::DiscordEmbed(embed)) => {
                    // Send embed message
                    let embed = Self::create_serenity_embed(embed);
                    let builder = CreateMessage::new().add_embed(embed);
                    let _sent = channel_id.send_message(http, builder).await.map_err(|e| {
                        crate::error::MantaError::ExternalService {
                            source: format!("Discord send failed: {}", e),
                            cause: None,
                        }
                    })?;
                    return Ok(Id::new());
                }
                _ => message.content,
            };

            // Send text message
            let builder = CreateMessage::new().content(content);
            let sent = channel_id.send_message(http, builder).await.map_err(|e| {
                crate::error::MantaError::ExternalService {
                    source: format!("Discord send failed: {}", e),
                    cause: None,
                }
            })?;

            // Track the message for edit/delete operations
            let message_id = sent.id.get();
            let channel_id_val = channel_id.get();
            self.track_message(message_id, channel_id_val).await;

            Ok(Id::new())
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = message;
            Err(crate::error::MantaError::Internal("Discord feature not enabled".to_string()))
        }
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> crate::Result<()> {
        #[cfg(feature = "discord")]
        {
            let channel_id: u64 = conversation_id.0.parse().map_err(|_| {
                crate::error::MantaError::Validation("Invalid channel ID".to_string())
            })?;

            let http = self.http.as_ref().ok_or_else(|| {
                crate::error::MantaError::Internal("HTTP client not initialized".to_string())
            })?;

            let channel_id = ChannelId::new(channel_id);

            // Trigger typing indicator
            channel_id.broadcast_typing(http).await.map_err(|e| {
                crate::error::MantaError::ExternalService {
                    source: format!("Discord typing indicator failed: {}", e),
                    cause: None,
                }
            })?;

            Ok(())
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = conversation_id;
            Err(crate::error::MantaError::Internal("Discord feature not enabled".to_string()))
        }
    }

    async fn edit_message(&self, message_id: Id, new_content: String) -> crate::Result<()> {
        #[cfg(feature = "discord")]
        {
            let message_id_num: u64 = message_id.to_string().parse().map_err(|_| {
                crate::error::MantaError::Validation("Invalid message ID".to_string())
            })?;

            // Look up the channel ID from our tracking map
            let channel_id_num = self.get_message_channel(message_id_num).await.ok_or_else(
                || crate::error::MantaError::NotFound {
                    resource: format!(
                        "Message {} not found in tracking (may have been sent before bot started)",
                        message_id_num
                    ),
                },
            )?;

            let http = self.http.as_ref().ok_or_else(|| {
                crate::error::MantaError::Internal("HTTP client not initialized".to_string())
            })?;

            let channel_id = ChannelId::new(channel_id_num);
            let message_id = serenity::model::id::MessageId::new(message_id_num);

            // Edit the message
            channel_id
                .edit_message(
                    http,
                    message_id,
                    serenity::builder::EditMessage::new().content(new_content),
                )
                .await
                .map_err(|e| crate::error::MantaError::ExternalService {
                    source: format!("Discord edit failed: {}", e),
                    cause: None,
                })?;

            Ok(())
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = (message_id, new_content);
            Err(crate::error::MantaError::Internal("Discord feature not enabled".to_string()))
        }
    }

    async fn delete_message(&self, message_id: Id) -> crate::Result<()> {
        #[cfg(feature = "discord")]
        {
            let message_id_num: u64 = message_id.to_string().parse().map_err(|_| {
                crate::error::MantaError::Validation("Invalid message ID".to_string())
            })?;

            // Look up the channel ID from our tracking map
            let channel_id_num = self.get_message_channel(message_id_num).await.ok_or_else(
                || crate::error::MantaError::NotFound {
                    resource: format!(
                        "Message {} not found in tracking (may have been sent before bot started)",
                        message_id_num
                    ),
                },
            )?;

            let http = self.http.as_ref().ok_or_else(|| {
                crate::error::MantaError::Internal("HTTP client not initialized".to_string())
            })?;

            let channel_id = ChannelId::new(channel_id_num);
            let message_id = serenity::model::id::MessageId::new(message_id_num);

            // Delete the message
            channel_id
                .delete_message(http, message_id)
                .await
                .map_err(|e| crate::error::MantaError::ExternalService {
                    source: format!("Discord delete failed: {}", e),
                    cause: None,
                })?;

            // Remove from tracking
            let mut map = self.message_channel_map.write().await;
            map.remove(&message_id_num);

            Ok(())
        }

        #[cfg(not(feature = "discord"))]
        {
            let _ = message_id;
            Err(crate::error::MantaError::Internal("Discord feature not enabled".to_string()))
        }
    }

    async fn health_check(&self) -> crate::Result<bool> {
        #[cfg(feature = "discord")]
        {
            if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(false);
            }

            // Check HTTP client is available and can fetch current user
            if let Some(http) = &self.http {
                match http.get_current_user().await {
                    Ok(_) => Ok(true),
                    Err(e) => {
                        warn!("Discord health check failed: {}", e);
                        Ok(false)
                    }
                }
            } else {
                Ok(false)
            }
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

        debug!("Received Discord message from {}: {}", msg.author.name, msg.content);

        // Handle DMs and mentions
        let is_dm = msg.guild_id.is_none();
        let is_mentioned = msg.mentions.iter().any(|u| u.bot);
        let has_prefix = msg.content.starts_with(&self.config.command_prefix);

        if is_dm || is_mentioned || has_prefix {
            let content = if has_prefix {
                msg.content[self.config.command_prefix.len()..]
                    .trim()
                    .to_string()
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

    /// Handle reaction additions
    async fn reaction_add(&self, ctx: Context, add_reaction: serenity::model::channel::Reaction) {
        debug!(
            "Reaction added: {:?} by user {}",
            add_reaction.emoji,
            add_reaction.user_id.map(|id| id.get()).unwrap_or(0)
        );

        // Get message info
        if let Ok(message) = add_reaction.message(&ctx.http).await {
            // Check if user is allowed
            let user_id_str = if let Some(user_id) = add_reaction.user_id {
                if !self.config.allowed_user_ids.is_empty()
                    && !self.config.allowed_user_ids.contains(&user_id.get())
                {
                    return;
                }
                user_id.get().to_string()
            } else {
                String::new()
            };

            // Create incoming message for reaction
            let reaction_content =
                format!("reaction_add:{}", reaction_emoji_name(&add_reaction.emoji));
            let incoming = IncomingMessage::new(
                &user_id_str,
                &add_reaction.channel_id.get().to_string(),
                reaction_content,
            )
            .with_metadata(
                MessageMetadata::new()
                    .with_extra("message_id", add_reaction.message_id.get())
                    .with_extra("reaction_emoji", reaction_emoji_name(&add_reaction.emoji))
                    .with_extra("reaction_type", "add")
                    .with_extra("original_message", message.content.clone()),
            );

            // Send to handler if configured
            if let Some(tx) = &self.config.message_tx {
                let _ = tx.send(incoming);
            }
        }
    }

    /// Handle reaction removals
    async fn reaction_remove(
        &self,
        _ctx: Context,
        removed_reaction: serenity::model::channel::Reaction,
    ) {
        debug!("Reaction removed: {:?}", removed_reaction.emoji);

        // Create incoming message for reaction removal
        let reaction_content =
            format!("reaction_remove:{}", reaction_emoji_name(&removed_reaction.emoji));
        let incoming = IncomingMessage::new(
            &removed_reaction
                .user_id
                .map(|id| id.get().to_string())
                .unwrap_or_default(),
            &removed_reaction.channel_id.get().to_string(),
            reaction_content,
        )
        .with_metadata(
            MessageMetadata::new()
                .with_extra("message_id", removed_reaction.message_id.get())
                .with_extra("reaction_emoji", reaction_emoji_name(&removed_reaction.emoji))
                .with_extra("reaction_type", "remove"),
        );

        // Send to handler if configured
        if let Some(tx) = &self.config.message_tx {
            let _ = tx.send(incoming);
        }
    }

    /// Handle all reactions being removed from a message
    async fn reaction_remove_all(
        &self,
        _ctx: Context,
        channel_id: serenity::model::id::ChannelId,
        message_id: serenity::model::id::MessageId,
    ) {
        debug!("All reactions removed from message {}", message_id);

        let incoming = IncomingMessage::new(
            "system",
            &channel_id.get().to_string(),
            "reaction_remove_all".to_string(),
        )
        .with_metadata(
            MessageMetadata::new()
                .with_extra("message_id", message_id.get())
                .with_extra("reaction_type", "remove_all"),
        );

        if let Some(tx) = &self.config.message_tx {
            let _ = tx.send(incoming);
        }
    }
}

/// Get emoji name for reaction
#[cfg(feature = "discord")]
fn reaction_emoji_name(emoji: &ReactionType) -> String {
    match emoji {
        ReactionType::Unicode(s) => s.clone(),
        ReactionType::Custom { animated: _, id, name } => {
            name.clone().unwrap_or_else(|| id.get().to_string())
        }
        _ => "unknown".to_string(),
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
