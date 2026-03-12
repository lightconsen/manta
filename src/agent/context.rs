//! Conversation context management for the Agent
//!
//! Context handles message history, token counting, and pruning
//! to keep conversations within the context window.

use crate::providers::Message;
use std::time::SystemTime;

/// Conversation context
#[derive(Debug, Clone)]
pub struct Context {
    /// Conversation ID
    id: String,
    /// System prompt
    system_prompt: String,
    /// Message history (excluding system prompt)
    messages: Vec<Message>,
    /// Maximum tokens allowed
    max_tokens: usize,
    /// Approximate token count
    token_count: usize,
    /// When the context was created
    created_at: SystemTime,
    /// When the context was last accessed
    last_accessed: SystemTime,
}

impl Context {
    /// Create a new context
    pub fn new(id: impl Into<String>, system_prompt: impl Into<String>, max_tokens: usize) -> Self {
        let now = SystemTime::now();
        Self {
            id: id.into(),
            system_prompt: system_prompt.into(),
            messages: Vec::new(),
            max_tokens,
            token_count: 0,
            created_at: now,
            last_accessed: now,
        }
    }

    /// Get the context ID
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the system prompt
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Add a message to the context
    pub fn add_message(&mut self, message: Message) {
        // Estimate tokens (rough approximation: 4 chars per token)
        let estimated_tokens = message.content.len() / 4;
        self.token_count += estimated_tokens;
        self.messages.push(message);
        self.last_accessed = SystemTime::now();

        // Prune if necessary
        self.prune_if_needed();
    }

    /// Get all messages including system prompt
    pub fn to_messages(&self) -> Vec<Message> {
        let mut result = Vec::with_capacity(self.messages.len() + 1);
        result.push(Message::system(&self.system_prompt));
        result.extend(self.messages.iter().cloned());
        result
    }

    /// Get message history (excluding system)
    pub fn history(&self) -> &[Message] {
        &self.messages
    }

    /// Clear the conversation history (keep system)
    pub fn clear(&mut self) {
        self.messages.clear();
        self.token_count = 0;
        self.last_accessed = SystemTime::now();
    }

    /// Get the approximate token count
    pub fn token_count(&self) -> usize {
        // Include system prompt
        self.token_count + (self.system_prompt.len() / 4)
    }

    /// Check if context needs pruning
    pub fn needs_pruning(&self) -> bool {
        self.token_count() > self.max_tokens
    }

    /// Prune messages to fit within token limit
    fn prune_if_needed(&mut self) {
        // Keep pruning until we're under the limit
        while self.token_count() > self.max_tokens && self.messages.len() > 1 {
            // Remove oldest non-system message (but keep the most recent user message)
            if self.messages.len() > 2 {
                let removed = self.messages.remove(0);
                self.token_count = self.token_count.saturating_sub(removed.content.len() / 4);
            } else {
                // If only 1-2 messages, just clear all but the last
                let last = self.messages.pop();
                self.messages.clear();
                if let Some(msg) = last {
                    self.messages.push(msg);
                }
                self.recalculate_tokens();
                break;
            }
        }
    }

    /// Recalculate token count from scratch
    fn recalculate_tokens(&mut self) {
        self.token_count = self.messages.iter().map(|m| m.content.len() / 4).sum();
    }

    /// Get the last message
    pub fn last_message(&self) -> Option<&Message> {
        self.messages.last()
    }

    /// Get when the context was last accessed
    pub fn last_accessed(&self) -> SystemTime {
        self.last_accessed
    }

    /// Get the age of the context
    pub fn age(&self) -> std::time::Duration {
        self.created_at
            .elapsed()
            .unwrap_or(std::time::Duration::from_secs(0))
    }

    /// Check if the context is stale (inactive for too long)
    pub fn is_stale(&self, max_age: std::time::Duration) -> bool {
        self.last_accessed
            .elapsed()
            .map(|elapsed| elapsed > max_age)
            .unwrap_or(true)
    }

    /// Get the number of messages
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Summarize the conversation (for very long contexts)
    pub fn summarize(&mut self) {
        // Keep first few and last few messages, summarize middle
        if self.messages.len() > 10 {
            let keep_first = 2;
            let keep_last = 4;
            let middle_start = keep_first;
            let middle_end = self.messages.len() - keep_last;

            if middle_end > middle_start {
                // Remove middle messages
                let _middle: Vec<Message> = self
                    .messages
                    .drain(middle_start..middle_end)
                    .collect();

                // Add summary placeholder
                let summary_msg = Message::system(
                    format!("[{} earlier messages omitted]", middle_end - middle_start)
                );
                self.messages.insert(middle_start, summary_msg);

                self.recalculate_tokens();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_creation() {
        let ctx = Context::new("test-123", "You are helpful.", 4096);
        assert_eq!(ctx.id(), "test-123");
        assert_eq!(ctx.system_prompt(), "You are helpful.");
        assert_eq!(ctx.message_count(), 0);
    }

    #[test]
    fn test_add_message() {
        let mut ctx = Context::new("test", "System", 1000);
        ctx.add_message(Message::user("Hello"));
        assert_eq!(ctx.message_count(), 1);
        assert!(ctx.last_message().is_some());
    }

    #[test]
    fn test_to_messages() {
        let mut ctx = Context::new("test", "System prompt", 1000);
        ctx.add_message(Message::user("Hello"));
        ctx.add_message(Message::assistant("Hi!"));

        let messages = ctx.to_messages();
        assert_eq!(messages.len(), 3); // System + user + assistant
        assert_eq!(messages[0].role, Role::System);
    }

    #[test]
    fn test_clear() {
        let mut ctx = Context::new("test", "System", 1000);
        ctx.add_message(Message::user("Hello"));
        ctx.clear();
        assert_eq!(ctx.message_count(), 0);
    }

    #[test]
    fn test_pruning() {
        // Create context with very small token limit to force pruning
        let mut ctx = Context::new("test", "S", 10); // ~2 tokens for system

        // Add many messages to trigger pruning
        for i in 0..20 {
            ctx.add_message(Message::user(format!("Message {} with some content", i)));
        }

        // Should have pruned to keep under limit
        assert!(ctx.token_count() <= ctx.max_tokens);
        assert!(ctx.message_count() < 20);
    }

    #[test]
    fn test_summarize() {
        let mut ctx = Context::new("test", "System", 10000);

        // Add many messages
        for i in 0..15 {
            ctx.add_message(Message::user(format!("Message {}", i)));
            ctx.add_message(Message::assistant(format!("Response {}", i)));
        }

        let before_count = ctx.message_count();
        ctx.summarize();

        // Should have fewer messages after summarization
        assert!(ctx.message_count() < before_count);
    }
}
