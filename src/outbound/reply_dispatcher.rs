//! Reply Dispatcher
//!
//! Routes agent responses back to the correct channel endpoint.
//! This replaces the ad-hoc `channel.send_message()` calls scattered
//! through the agent loop with a unified dispatch layer.
//!
//! Design matches OpenClaw's `src/reply-dispatcher/`.

use crate::channels::{Channel, OutgoingMessage};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Configuration for reply dispatch.
#[derive(Debug, Clone)]
pub struct ReplyDispatchConfig {
    /// Whether to split long messages into chunks.
    pub chunk_long_messages: bool,
    /// Maximum message length before chunking.
    pub max_chunk_length: usize,
    /// Whether to suppress empty replies.
    pub suppress_empty: bool,
}

impl Default for ReplyDispatchConfig {
    fn default() -> Self {
        Self {
            chunk_long_messages: true,
            max_chunk_length: 4000,
            suppress_empty: true,
        }
    }
}

/// Reply dispatcher routes outbound messages to channels.
pub struct ReplyDispatcher {
    config: ReplyDispatchConfig,
    /// channel_name -> Channel handle
    channels: RwLock<HashMap<String, Arc<dyn Channel>>>,
}

impl ReplyDispatcher {
    pub fn new(config: ReplyDispatchConfig) -> Self {
        Self {
            config,
            channels: RwLock::new(HashMap::new()),
        }
    }

    /// Register a channel for dispatch.
    pub async fn register_channel(&self, name: &str, channel: Arc<dyn Channel>) {
        let mut channels = self.channels.write().await;
        channels.insert(name.to_string(), channel);
        info!("Registered channel '{}' for reply dispatch", name);
    }

    /// Dispatch an outgoing message to its target channel.
    pub async fn dispatch(
        &self,
        channel_name: &str,
        message: OutgoingMessage,
    ) -> Result<(), ReplyDispatchError> {
        if self.config.suppress_empty && message.content.trim().is_empty() {
            debug!("Suppressing empty reply to {}", channel_name);
            return Ok(());
        }

        let channels = self.channels.read().await;
        let channel = channels
            .get(channel_name)
            .ok_or_else(|| ReplyDispatchError::ChannelNotFound(channel_name.to_string()))?;

        let content = if self.config.chunk_long_messages
            && message.content.len() > self.config.max_chunk_length
        {
            // For now, just send the full message.  Future: chunk it.
            message.content.clone()
        } else {
            message.content.clone()
        };

        let msg = OutgoingMessage { content, ..message };

        debug!(
            "Dispatching reply to channel {} (conversation {})",
            channel_name, msg.conversation_id.0
        );

        match channel.send(msg).await {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("Failed to dispatch reply to {}: {}", channel_name, e);
                Err(ReplyDispatchError::SendFailed(e.to_string()))
            }
        }
    }

    /// List registered channels.
    pub async fn list_channels(&self) -> Vec<String> {
        let channels = self.channels.read().await;
        channels.keys().cloned().collect()
    }
}

/// Errors from the reply dispatcher.
#[derive(Debug, thiserror::Error)]
pub enum ReplyDispatchError {
    #[error("Channel not found: {0}")]
    ChannelNotFound(String),
    #[error("Failed to send message: {0}")]
    SendFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{ConversationId, MessageOptions, OutgoingMessage};

    fn dummy_msg(content: impl Into<String>) -> OutgoingMessage {
        OutgoingMessage {
            conversation_id: ConversationId::new("test"),
            content: content.into(),
            formatted_content: None,
            attachments: vec![],
            reply_to: None,
            options: MessageOptions {
                silent: false,
                show_typing: false,
                custom: std::collections::HashMap::new(),
            },
            usage: None,
        }
    }

    #[tokio::test]
    async fn test_dispatch_empty_suppressed() {
        let dispatcher = ReplyDispatcher::new(ReplyDispatchConfig::default());
        let msg = dummy_msg("   ");
        // No channel registered, but empty should be suppressed first.
        let result = dispatcher.dispatch("test", msg).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_channel_not_found() {
        let dispatcher = ReplyDispatcher::new(ReplyDispatchConfig::default());
        let msg = dummy_msg("hello");
        let result = dispatcher.dispatch("missing", msg).await;
        assert!(matches!(result, Err(ReplyDispatchError::ChannelNotFound(_))));
    }
}
