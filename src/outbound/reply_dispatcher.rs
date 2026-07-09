//! Reply Dispatcher
//!
//! Routes agent responses back to the correct channel endpoint.
//! This replaces the ad-hoc `channel.send_message()` calls scattered
//! through the agent loop with a unified dispatch layer.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, error, info};

use crate::channels::{Channel, OutgoingMessage};

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

    /// Remove a previously registered channel.
    pub async fn unregister_channel(&self, name: &str) {
        let mut channels = self.channels.write().await;
        channels.remove(name);
        info!("Unregistered channel '{}' from reply dispatch", name);
    }

    /// Dispatch an outgoing message to its target channel.
    /// Long messages are split into chunks at word boundaries.
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

        let chunks = if self.config.chunk_long_messages
            && message.content.len() > self.config.max_chunk_length
        {
            chunk_content(&message.content, self.config.max_chunk_length)
        } else {
            vec![message.content.clone()]
        };

        for content in chunks {
            let msg = OutgoingMessage { content, ..message.clone() };
            debug!(
                "Dispatching reply to channel {} (conversation {})",
                channel_name, msg.conversation_id.0
            );
            if let Err(e) = channel.send(msg).await {
                error!("Failed to dispatch reply to {}: {}", channel_name, e);
                return Err(ReplyDispatchError::SendFailed(e.to_string()));
            }
        }
        Ok(())
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

/// Split content into chunks at word boundaries, each at most `max_len` bytes.
///
/// Each chunk ends at a space character when possible to avoid splitting words.
/// If a single word exceeds `max_len`, it is hard-split at the limit.
fn chunk_content(content: &str, max_len: usize) -> Vec<String> {
    if max_len == 0 {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = content;

    while remaining.len() > max_len {
        // Find the last space within the first max_len characters
        let slice = &remaining[..max_len];
        let split_at = slice.rfind(' ').map(|pos| pos + 1).unwrap_or(max_len);
        chunks.push(remaining[..split_at].to_string());
        remaining = remaining[split_at..].trim_start();
    }

    if !remaining.is_empty() {
        chunks.push(remaining.to_string());
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::{ConversationId, MessageOptions, OutgoingMessage};

    fn dummy_msg(content: impl Into<String>) -> OutgoingMessage {
        OutgoingMessage {
            conversation_id: ConversationId::new("test"),
            content: content.into(),
            reasoning_content: None,
            tool_calls: None,
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

    // ── chunk_content tests ─────────────────────────────────────────────

    #[test]
    fn test_chunk_content_no_split() {
        let chunks = chunk_content("hello world", 100);
        assert_eq!(chunks, vec!["hello world"]);
    }

    #[test]
    fn test_chunk_content_word_boundary() {
        // "hello worl"[..10] = "hello worl", rfind(' ') = 5 → "hello " + "world foo
        // bar"
        let chunks = chunk_content("hello world foo bar", 10);
        assert_eq!(chunks, vec!["hello ", "world foo ", "bar"]);
    }

    #[test]
    fn test_chunk_content_no_space() {
        let chunks = chunk_content("abcdefghijklmnop", 5);
        assert_eq!(chunks, vec!["abcde", "fghij", "klmno", "p"]);
    }

    #[test]
    fn test_chunk_content_empty() {
        let chunks = chunk_content("", 10);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_content_exact_fit() {
        let chunks = chunk_content("12345", 5);
        assert_eq!(chunks, vec!["12345"]);
    }

    #[test]
    fn test_chunk_content_max_len_zero() {
        let chunks = chunk_content("hello", 0);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn test_chunk_content_trim_remainder() {
        let chunks = chunk_content("12345 7890", 5);
        // "12345"[..5].rfind(' ') = None → split_at = 5 → "12345" + " 7890"
        // trim_start → "7890"
        assert_eq!(chunks, vec!["12345", "7890"]);
    }

    // ── dispatcher tests ────────────────────────────────────────────────

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

    #[tokio::test]
    async fn test_list_channels() {
        let dispatcher = ReplyDispatcher::new(ReplyDispatchConfig::default());
        assert!(dispatcher.list_channels().await.is_empty());
    }

    #[tokio::test]
    async fn test_config_default() {
        let config = ReplyDispatchConfig::default();
        assert!(config.chunk_long_messages);
        assert_eq!(config.max_chunk_length, 4000);
        assert!(config.suppress_empty);
    }

    #[tokio::test]
    async fn test_dispatch_empty_not_suppressed() {
        let mut config = ReplyDispatchConfig::default();
        config.suppress_empty = false;
        let dispatcher = ReplyDispatcher::new(config);
        let msg = dummy_msg("   ");
        let result = dispatcher.dispatch("test", msg).await;
        assert!(matches!(result, Err(ReplyDispatchError::ChannelNotFound(_))));
    }
}
