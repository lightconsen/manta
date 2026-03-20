//! Context Compression for managing long conversations
//!
//! This module implements context window management by compressing
//! messages when approaching token limits.
//!
//! In addition to the heuristic strategies (`OldestFirst`, `Summarize`,
//! `SlidingWindow`), a `compact_with_llm` helper is provided that asks an
//! LLM provider to write a concise summary of the mid-section of the history
//! so recent context is preserved while tokens are freed.

use crate::providers::{Message, Provider, Role};
use std::sync::Arc;
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

        info!("Compressing context: {} -> ~{} tokens", current_tokens, self.target_tokens);

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
            if total_tokens + tokens <= self.target_tokens
                || pm.priority == MessagePriority::Critical
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

        info!("Summarized {} messages into {} messages", messages.len(), result.len());

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

        info!("Sliding window: kept {} of {} messages", result.len(), messages.len());

        result
    }

    /// Create a heuristic summary message from a set of messages.
    ///
    /// Groups turns into user-request / assistant-response pairs and produces a
    /// compact digest that preserves the key intent of each exchange.  This is a
    /// best-effort sync fallback; for LLM-quality summarization use
    /// [`compact_with_llm`].
    fn create_summary(&self, messages: &[Message]) -> Message {
        let mut lines: Vec<String> = Vec::new();

        let non_empty: Vec<&Message> = messages
            .iter()
            .filter(|m| !m.content.is_empty())
            .collect();

        let mut i = 0;
        while i < non_empty.len() {
            let msg = non_empty[i];
            match msg.role {
                Role::User => {
                    // User turn: capture the request intent (up to 150 chars)
                    let preview: String = msg.content.chars().take(150).collect();
                    let ellipsis = if msg.content.len() > 150 { "…" } else { "" };
                    lines.push(format!("Q: {}{}", preview, ellipsis));

                    // Peek at the following assistant turn if present
                    if i + 1 < non_empty.len() && non_empty[i + 1].role == Role::Assistant {
                        let resp = non_empty[i + 1];
                        let preview: String = resp.content.chars().take(250).collect();
                        let ellipsis = if resp.content.len() > 250 { "…" } else { "" };
                        lines.push(format!("A: {}{}", preview, ellipsis));
                        i += 2;
                        continue;
                    }
                }
                Role::Assistant => {
                    let preview: String = msg.content.chars().take(250).collect();
                    let ellipsis = if msg.content.len() > 250 { "…" } else { "" };
                    lines.push(format!("A: {}{}", preview, ellipsis));
                }
                _ => {
                    // Skip tool calls and other roles in the summary
                }
            }
            i += 1;
        }

        let content = format!(
            "[Summary of {} previous messages]\n{}",
            messages.len(),
            lines.join("\n")
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

    /// Compact `messages` using an LLM to summarise the mid-section.
    ///
    /// Keeps the first `keep_head` and last `keep_tail` messages intact and
    /// asks `provider` to summarise everything in-between.  Returns the
    /// compacted list on success; on any provider error the original messages
    /// are returned unchanged (graceful degradation).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use std::sync::Arc;
    /// # use manta::agent::compressor::ContextCompressor;
    /// # async fn example(provider: Arc<dyn manta::providers::Provider>, messages: Vec<manta::providers::Message>) {
    /// let compressor = ContextCompressor::new(4096);
    /// let compacted = compressor.compact_with_llm(&messages, &provider, None, 2, 6).await;
    /// # }
    /// ```
    pub async fn compact_with_llm(
        &self,
        messages: &[Message],
        provider: &Arc<dyn Provider>,
        model: Option<&str>,
        keep_head: usize,
        keep_tail: usize,
    ) -> Vec<Message> {
        let n = messages.len();

        // Nothing to summarise if the history is too short.
        let mid_start = keep_head;
        let mid_end = n.saturating_sub(keep_tail);
        if mid_start >= mid_end {
            debug!("compact_with_llm: history too short to summarise, returning as-is");
            return messages.to_vec();
        }

        let head = &messages[..mid_start];
        let mid = &messages[mid_start..mid_end];
        let tail = &messages[mid_end..];

        // Build a compact transcript of the mid section for the LLM prompt.
        let transcript: String = mid
            .iter()
            .filter(|m| !m.content.is_empty())
            .map(|m| format!("{}: {}", m.role, m.content.chars().take(400).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "Summarise the following conversation excerpt in ≤150 words, preserving key \
             facts, decisions, and named entities. Output only the summary text.\n\n{}",
            transcript
        );

        let req = crate::providers::CompletionRequest {
            model: model.map(str::to_string),
            messages: vec![Message::user(prompt)],
            max_tokens: Some(300),
            temperature: Some(0.3),
            tools: None,
            stream: false,
            stop: None,
        };

        match provider.complete(req).await {
            Ok(response) => {
                let summary_text = response.message.content;
                if summary_text.is_empty() {
                    warn!("compact_with_llm: provider returned empty summary, skipping");
                    return messages.to_vec();
                }

                info!(
                    "compact_with_llm: summarised {} messages into {} chars",
                    mid.len(),
                    summary_text.len()
                );

                let summary_msg = Message {
                    role: Role::System,
                    content: format!(
                        "[Summary of {} earlier messages]\n{}",
                        mid.len(),
                        summary_text
                    ),
                    name: Some("compaction_summary".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                    metadata: None,
                };

                let mut result = head.to_vec();
                result.push(summary_msg);
                result.extend_from_slice(tail);
                result
            }
            Err(e) => {
                warn!("compact_with_llm: provider error (returning original): {}", e);
                messages.to_vec()
            }
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
        // Use more messages to ensure we exceed the threshold
        assert!(compressor.needs_compression(&create_test_messages(150)));
    }

    #[test]
    fn test_sliding_window() {
        let compressor =
            ContextCompressor::new(100).with_strategy(CompressionStrategy::SlidingWindow);
        let messages = create_test_messages(20);
        let compressed = compressor.compress(&messages);

        assert!(compressed.len() < messages.len());
        // System message should be preserved
        assert!(compressed.iter().any(|m| m.role == Role::System));
    }

    #[test]
    fn test_oldest_first() {
        let compressor =
            ContextCompressor::new(150).with_strategy(CompressionStrategy::OldestFirst);
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
