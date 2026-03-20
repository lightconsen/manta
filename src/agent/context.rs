//! Conversation context management for the Agent
//!
//! Context handles message history, token counting, and pruning
//! to keep conversations within the context window.

use crate::providers::Message;
use std::collections::HashSet;
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
    /// Tool call iteration counter (to prevent infinite loops)
    tool_iterations: usize,
    /// Maximum allowed tool iterations
    max_tool_iterations: usize,
    /// Track tool calls to prevent duplicates (tool_name + params_hash)
    executed_tool_calls: HashSet<String>,
    /// Optional hard cap on the number of turns kept in history.
    /// When set, `add_message` enforces this limit by dropping the oldest
    /// user/assistant pairs before the token-based prune runs.
    max_turns: Option<usize>,
}

impl Context {
    /// Default maximum tool iterations to prevent infinite loops
    pub const DEFAULT_MAX_TOOL_ITERATIONS: usize = 10;

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
            tool_iterations: 0,
            max_tool_iterations: Self::DEFAULT_MAX_TOOL_ITERATIONS,
            executed_tool_calls: HashSet::new(),
            max_turns: None,
        }
    }

    /// Set a hard cap on conversation turns (user+assistant pairs).
    ///
    /// When the history grows beyond `turns` pairs the oldest pair is dropped
    /// before the normal token-based pruning runs.  A "turn" is counted as one
    /// user message (tool messages are not counted as separate turns).
    pub fn with_max_turns(mut self, turns: usize) -> Self {
        self.max_turns = Some(turns);
        self
    }

    /// Enforce the turn limit by dropping oldest messages until the user-message
    /// count is within `max_turns`.  Tool (result) messages paired with an
    /// assistant message are removed together to keep the conversation coherent.
    pub fn limit_turns(&mut self) {
        let Some(max) = self.max_turns else { return };

        loop {
            // Count user-role messages as turn markers.
            let user_count = self
                .messages
                .iter()
                .filter(|m| m.role == crate::providers::Role::User)
                .count();

            if user_count <= max {
                break;
            }

            // Find and remove the oldest user message plus any immediately
            // following tool/assistant messages that belong to that turn.
            if let Some(oldest_user) = self
                .messages
                .iter()
                .position(|m| m.role == crate::providers::Role::User)
            {
                let removed = self.messages.remove(oldest_user);
                self.token_count = self.token_count.saturating_sub(removed.content.len() / 4);

                // Also remove the assistant reply that follows (if present).
                if oldest_user < self.messages.len() {
                    let next = &self.messages[oldest_user];
                    if next.role == crate::providers::Role::Assistant {
                        let removed = self.messages.remove(oldest_user);
                        self.token_count =
                            self.token_count.saturating_sub(removed.content.len() / 4);
                    }
                }
            } else {
                break;
            }
        }
    }

    /// Check if a tool call with these parameters was already executed
    pub fn is_tool_call_duplicate(&self, tool_name: &str, params: &str) -> bool {
        let key = format!("{}:{}", tool_name, params);
        self.executed_tool_calls.contains(&key)
    }

    /// Record a tool call as executed
    pub fn record_tool_call(&mut self, tool_name: &str, params: &str) {
        let key = format!("{}:{}", tool_name, params);
        self.executed_tool_calls.insert(key);
    }

    /// Increment tool iteration counter
    /// Returns false if limit reached
    pub fn increment_tool_iteration(&mut self) -> bool {
        self.tool_iterations += 1;
        self.tool_iterations < self.max_tool_iterations
    }

    /// Calculate dynamic tool limit based on task complexity
    /// Can be overridden by MANTA_MAX_TOOL_ITERATIONS env var
    pub fn calculate_dynamic_limit(message_content: &str) -> usize {
        // Check for env var override first
        if let Ok(limit) = std::env::var("MANTA_MAX_TOOL_ITERATIONS") {
            if let Ok(parsed) = limit.parse::<usize>() {
                return parsed;
            }
        }

        // Base limit: 10
        let base_limit = 10;

        // Scale based on message complexity indicators
        let mut complexity = 0;

        // Longer queries may need more steps
        if message_content.len() > 200 {
            complexity += 5;
        }

        // Multi-part tasks (indicated by "and", "then", commas)
        let parts = message_content.split(|c| c == ',' || c == ';').count();
        complexity += parts.saturating_sub(1) * 2;

        // Tasks with explicit multiple steps
        if message_content.to_lowercase().contains("steps")
            || message_content.to_lowercase().contains("step by step")
        {
            complexity += 5;
        }

        // Cap at reasonable maximum (30)
        std::cmp::min(30, base_limit + complexity)
    }

    /// Get current tool iteration count
    pub fn tool_iterations(&self) -> usize {
        self.tool_iterations
    }

    /// Check if tool iteration limit is reached
    pub fn is_tool_limit_reached(&self) -> bool {
        self.tool_iterations >= self.max_tool_iterations
    }

    /// Set maximum tool iterations
    pub fn set_max_tool_iterations(&mut self, max: usize) {
        self.max_tool_iterations = max;
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

        // Enforce turn limit before token-based pruning.
        self.limit_turns();

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
    /// Note: This preserves tool call pairs (assistant message with tool_use + tool result)
    fn prune_if_needed(&mut self) {
        // Collect tool call IDs that have corresponding tool results
        // These must not be pruned or the API will error
        let pending_tool_call_ids: std::collections::HashSet<String> = self
            .messages
            .iter()
            .filter(|m| m.role == crate::providers::Role::Tool)
            .filter_map(|m| m.tool_call_id.clone())
            .collect();

        // Keep pruning until we're under the limit
        while self.token_count() > self.max_tokens && self.messages.len() > 1 {
            // Find the oldest message that can be safely pruned
            let prune_index = self.find_prunable_message(&pending_tool_call_ids);

            if let Some(index) = prune_index {
                let removed = self.messages.remove(index);
                self.token_count = self.token_count.saturating_sub(removed.content.len() / 4);
            } else {
                // Can't prune anything safely, break to avoid infinite loop
                break;
            }
        }
    }

    /// Find the index of a message that can be safely pruned
    /// Returns None if no message can be pruned (all are protected)
    fn find_prunable_message(
        &self,
        pending_tool_call_ids: &std::collections::HashSet<String>,
    ) -> Option<usize> {
        // Try to find the oldest message that is not a protected tool call pair
        for (index, msg) in self.messages.iter().enumerate() {
            // Never prune the last message
            if index == self.messages.len() - 1 {
                return None;
            }

            // Check if this is an assistant message with tool calls
            if msg.role == crate::providers::Role::Assistant {
                if let Some(ref tool_calls) = msg.tool_calls {
                    // Check if any of these tool calls have pending results
                    let has_pending_results = tool_calls
                        .iter()
                        .any(|tc| pending_tool_call_ids.contains(&tc.id));

                    if has_pending_results {
                        // Can't prune this message - it has pending tool results
                        continue;
                    }
                }
            }

            // Check if this is a tool result message
            if msg.role == crate::providers::Role::Tool {
                // Can't prune tool results without also pruning the assistant message
                // that made the call (which we check above). For now, keep tool results.
                // A more sophisticated approach would prune both together.
                continue;
            }

            // This message can be pruned
            return Some(index);
        }

        None
    }

    /// Replace the message history with a compacted set (e.g. after LLM-assisted compaction).
    ///
    /// The system prompt is stored separately and is unaffected.  Token count
    /// is recalculated from the new message list.
    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.recalculate_tokens();
        self.last_accessed = SystemTime::now();
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
                let _middle: Vec<Message> = self.messages.drain(middle_start..middle_end).collect();

                // Add summary placeholder
                let summary_msg = Message::system(format!(
                    "[{} earlier messages omitted]",
                    middle_end - middle_start
                ));
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
        use crate::providers::Role;
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
    fn test_max_turns_limits_history() {
        let mut ctx = Context::new("test", "System", 100_000).with_max_turns(2);

        // Add 3 user+assistant pairs.
        for i in 0..3 {
            ctx.add_message(Message::user(format!("User {}", i)));
            ctx.add_message(Message::assistant(format!("Assistant {}", i)));
        }

        // Only the 2 most recent user messages should remain.
        let user_count = ctx
            .history()
            .iter()
            .filter(|m| m.role == crate::providers::Role::User)
            .count();
        assert_eq!(user_count, 2);
    }

    #[test]
    fn test_max_turns_none_no_drop() {
        let mut ctx = Context::new("test", "System", 100_000);
        // No max_turns set — all messages kept.
        for i in 0..5 {
            ctx.add_message(Message::user(format!("User {}", i)));
            ctx.add_message(Message::assistant(format!("Assistant {}", i)));
        }
        assert_eq!(ctx.message_count(), 10);
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
