//! Context Compression for managing long conversations
//!
//! This module implements context window management by compressing
//! messages when approaching token limits.

use crate::providers::{Message, Role};
use tracing::{debug, info, warn};

/// Estimated tokens per character (approximation)
const TOKENS_PER_CHAR: f32 = 0.25;

/// Priority levels for messages during compression
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessagePriority {
    /// Critical - never remove (system prompts, todos)
    Critical = 3,
    /// High - prefer to keep (recent messages, tool results)
    High = 2,
    /// Normal - can be summarized (assistant responses)
    Normal = 1,
    /// Low - can be removed (old user messages)
    Low = 0,
}

/// A message with priority metadata
#[derive(Debug, Clone)]
pub struct PrioritizedMessage {
    /// The message
    pub message: Message,
    /// Priority level
    pub priority: MessagePriority,
    /// Original index
    pub index: usize,
    /// Whether this message has been summarized
    pub summarized: bool,
}

impl PrioritizedMessage {
    /// Create a new prioritized message
    pub fn new(message: Message, index: usize) -> Self {
        let priority = Self::calculate_priority(&message, index);
        Self {
            message,
            priority,
            index,
            summarized: false,
        }
    }

    /// Calculate priority based on message content and position
    fn calculate_priority(message: &Message, index: usize) -> MessagePriority {
        // Recent messages (last 6) are high priority
        if index >= 6 {
            return MessagePriority::High;
        }

        match message.role {
            Role::System => MessagePriority::Critical,
            Role::Tool => MessagePriority::High,
            Role::Assistant => {
                // Check if contains important markers
                if message.content.contains("Task") || message.content.contains("todo") {
                    MessagePriority::High
                } else {
                    MessagePriority::Normal
                }
            }
            Role::User => {
                // Recent user messages are high priority
                if index >= 4 {
                    MessagePriority::High
                } else {
                    MessagePriority::Low
                }
            }
        }
    }

    /// Estimate token count
    pub fn estimated_tokens(&self) -> usize {
        (self.message.content.len() as f32 * TOKENS_PER_CHAR) as usize + 4
    }
}

/// Context compressor for managing token budget
#[derive(Debug, Clone)]
pub struct ContextCompressor {
    /// Target token count after compression
    target_tokens: usize,
    /// Minimum tokens to trigger compression
    compression_threshold: usize,
    /// Strategy for compression
    strategy: CompressionStrategy,
}

/// Compression strategies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionStrategy {
    /// Remove oldest low-priority messages first
    OldestFirst,
    /// Summarize groups of messages
    Summarize,
    /// Sliding window (keep only recent messages)
    SlidingWindow,
}

impl ContextCompressor {
    /// Create a new compressor with target token count
    pub fn new(target_tokens: usize) -> Self {
        Self {
            target_tokens,
            compression_threshold: (target_tokens as f32 * 1.2) as usize,
            strategy: CompressionStrategy::OldestFirst,
        }
    }

    /// Set compression threshold (as percentage of target)
    pub fn with_threshold(mut self, threshold_percent: f32) -> Self {
        self.compression_threshold = (self.target_tokens as f32 * threshold_percent) as usize;
        self
    }

    /// Set compression strategy
    pub fn with_strategy(mut self, strategy: CompressionStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Check if compression is needed
    pub fn needs_compression(&self, messages: &[Message]) -> bool {
        self.estimate_tokens(messages) > self.compression_threshold
    }

    /// Estimate total tokens for a set of messages
    pub fn estimate_tokens(&self, messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|m| (m.content.len() as f32 * TOKENS_PER_CHAR) as usize + 4)
            .sum()
    }

    /// Compress messages to target token count
    pub fn compress(&self, messages: &[Message]) -> Vec<Message> {
        let current_tokens = self.estimate_tokens(messages);

        if current_tokens <= self.target_tokens {
            debug!("No compression needed: {} <= {} tokens", current_tokens, self.target_tokens);
            return messages.to_vec();
        }

        info!(
            "Compressing context: {} -> ~{} tokens",
            current_tokens, self.target_tokens
        );

        match self.strategy {
            CompressionStrategy::OldestFirst => self.compress_oldest_first(messages),
            CompressionStrategy::Summarize => self.compress_summarize(messages),
            CompressionStrategy::SlidingWindow => self.compress_sliding_window(messages),
        }
    }

    /// Compress by removing oldest low-priority messages
    fn compress_oldest_first(&self, messages: &[Message]) -> Vec<Message> {
        // Create prioritized messages
        let mut prioritized: Vec<PrioritizedMessage> = messages
            .iter()
            .enumerate()
            .map(|(i, m)| PrioritizedMessage::new(m.clone(), i))
            .collect();

        // Sort by priority (desc) then index (desc) to keep recent high-priority
        prioritized.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| b.index.cmp(&a.index))
        });

        // Select messages until we hit the target
        let mut result = Vec::new();
        let mut total_tokens = 0;

        for pm in prioritized {
            let tokens = pm.estimated_tokens();
            if total_tokens + tokens <= self.target_tokens || pm.priority == MessagePriority::Critical
            {
                result.push(pm);
                total_tokens += tokens;
            }
        }

        // Sort back by original index
        result.sort_by_key(|pm| pm.index);

        info!(
            "Compressed from {} to {} messages (~{} tokens)",
            messages.len(),
            result.len(),
            total_tokens
        );

        result.into_iter().map(|pm| pm.message).collect()
    }

    /// Compress by summarizing message groups
    fn compress_summarize(&self, messages: &[Message]) -> Vec<Message> {
        // Keep system and recent messages
        let mut result = Vec::new();
        let mut to_summarize = Vec::new();

        for (i, msg) in messages.iter().enumerate() {
            if msg.role == Role::System || i >= messages.len().saturating_sub(4) {
                result.push(msg.clone());
            } else {
                to_summarize.push(msg.clone());
            }
        }

        // If we have messages to summarize, create a summary
        if !to_summarize.is_empty() {
            let summary = self.create_summary(&to_summarize);
            result.insert(1, summary);
        }

        info!(
            "Summarized {} messages into {} messages",
            messages.len(),
            result.len()
        );

        result
    }

    /// Compress using sliding window (keep only recent)
    fn compress_sliding_window(&self, messages: &[Message]) -> Vec<Message> {
        // Always keep system messages
        let system_messages: Vec<_> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .cloned()
            .collect();

        let system_tokens: usize = system_messages
            .iter()
            .map(|m| (m.content.len() as f32 * TOKENS_PER_CHAR) as usize)
            .sum();

        let available_tokens = self.target_tokens.saturating_sub(system_tokens);

        // Add recent messages until we hit the limit
        let mut recent = Vec::new();
        let mut total_tokens = 0;

        for msg in messages.iter().rev().filter(|m| m.role != Role::System) {
            let tokens = (msg.content.len() as f32 * TOKENS_PER_CHAR) as usize + 4;
            if total_tokens + tokens <= available_tokens {
                recent.push(msg.clone());
                total_tokens += tokens;
            } else {
                break;
            }
        }

        recent.reverse();

        let mut result = system_messages;
        result.extend(recent);

        info!(
            "Sliding window: kept {} of {} messages",
            result.len(),
            messages.len()
        );

        result
    }

    /// Create a summary message from a set of messages
    fn create_summary(&self, messages: &[Message]) -> Message {
        let summary_points: Vec<String> = messages
            .iter()
            .filter(|m| !m.content.is_empty())
            .take(10)
            .map(|m| format!("- {}: {}", m.role, m.content.chars().take(100).collect::<String>()))
            .collect();

        let content = format!(
            "[Summary of {} previous messages]\n{}",
            messages.len(),
            summary_points.join("\n")
        );

        Message {
            role: Role::System,
            content,
            name: Some("summary".to_string()),
            tool_calls: None,
            tool_call_id: None,
            metadata: None,
        }
    }

    /// Get compression statistics
    pub fn stats(&self, before: &[Message], after: &[Message]) -> CompressionStats {
        let before_tokens = self.estimate_tokens(before);
        let after_tokens = self.estimate_tokens(after);

        CompressionStats {
            before_messages: before.len(),
            after_messages: after.len(),
            before_tokens,
            after_tokens,
            reduction_percent: if before_tokens > 0 {
                ((before_tokens - after_tokens) as f32 / before_tokens as f32) * 100.0
            } else {
                0.0
            },
        }
    }
}

/// Compression statistics
#[derive(Debug, Clone)]
pub struct CompressionStats {
    /// Messages before compression
    pub before_messages: usize,
    /// Messages after compression
    pub after_messages: usize,
    /// Tokens before compression
    pub before_tokens: usize,
    /// Tokens after compression
    pub after_tokens: usize,
    /// Reduction percentage
    pub reduction_percent: f32,
}

impl std::fmt::Display for CompressionStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Compression: {} -> {} messages ({} -> {} tokens, {:.1}% reduction)",
            self.before_messages,
            self.after_messages,
            self.before_tokens,
            self.after_tokens,
            self.reduction_percent
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_messages(count: usize) -> Vec<Message> {
        (0..count)
            .map(|i| Message {
                role: if i == 0 { Role::System } else { Role::User },
                content: format!("Message {} with some content", i),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                metadata: None,
            })
            .collect()
    }

    #[test]
    fn test_compressor_creation() {
        let compressor = ContextCompressor::new(1000);
        assert_eq!(compressor.target_tokens, 1000);
        assert!(compressor.needs_compression(&create_test_messages(100)));
    }

    #[test]
    fn test_sliding_window() {
        let compressor = ContextCompressor::new(100).with_strategy(CompressionStrategy::SlidingWindow);
        let messages = create_test_messages(20);
        let compressed = compressor.compress(&messages);

        assert!(compressed.len() < messages.len());
        // System message should be preserved
        assert!(compressed.iter().any(|m| m.role == Role::System));
    }

    #[test]
    fn test_oldest_first() {
        let compressor = ContextCompressor::new(150).with_strategy(CompressionStrategy::OldestFirst);
        let messages = create_test_messages(20);
        let compressed = compressor.compress(&messages);

        assert!(compressed.len() <= messages.len());
        // System message should be preserved
        assert!(compressed.iter().any(|m| m.role == Role::System));
    }

    #[test]
    fn test_no_compression_needed() {
        let compressor = ContextCompressor::new(10000);
        let messages = create_test_messages(5);
        let compressed = compressor.compress(&messages);

        assert_eq!(compressed.len(), messages.len());
    }
}
