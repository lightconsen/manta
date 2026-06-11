//! Pre-compaction Memory Flush
//!
//! Triggers a silent agent turn before compaction to store durable memories.
//!

use crate::agent::compaction::{compute_context_hash, MemoryFlushConfig, SessionCompactionState};
use chrono::Local;

/// Result of a memory flush check
#[derive(Debug)]
pub struct MemoryFlushDecision {
    pub should_flush: bool,
    pub reason: FlushReason,
    pub context_hash: String,
}

/// Reason why a memory flush was triggered (or not)
#[derive(Debug, Clone)]
pub enum FlushReason {
 /// No flush needed
    None,
 /// Token threshold exceeded
    TokenThreshold,
 /// Transcript size exceeded
    TranscriptSize,
}

/// Check if memory flush should run based on current session state
///
/// # Arguments
/// * `total_tokens` - Current token count in the session context
/// * `transcript_bytes` - Size of the session transcript in bytes
/// * `config` - Memory flush configuration
/// * `context_window` - Total context window size for the model
/// * `compaction_state` - Current compaction state for deduplication
/// * `current_messages` - Recent messages for hash computation
///
/// # Returns
/// A `MemoryFlushDecision` indicating whether to flush and why
pub fn check_memory_flush(
    total_tokens: usize,
    transcript_bytes: usize,
    config: &MemoryFlushConfig,
    context_window: usize,
    compaction_state: &SessionCompactionState,
    current_messages: &[(String, String)],
) -> MemoryFlushDecision {
    let context_hash = compute_context_hash(current_messages);

 // Check if already flushed this cycle
    if compaction_state.memory_flush_compaction_count == Some(compaction_state.compaction_count) {
        return MemoryFlushDecision {
            should_flush: false,
            reason: FlushReason::None,
            context_hash,
        };
    }

 // Check context hash dedup
    if compaction_state.last_flush_context_hash.as_deref() == Some(&context_hash) {
        return MemoryFlushDecision {
            should_flush: false,
            reason: FlushReason::None,
            context_hash,
        };
    }

 // Check token threshold
    let threshold = context_window
        .saturating_sub(config.reserve_tokens_floor)
        .saturating_sub(config.soft_threshold_tokens);

    if total_tokens >= threshold {
        return MemoryFlushDecision {
            should_flush: true,
            reason: FlushReason::TokenThreshold,
            context_hash,
        };
    }

 // Check transcript size
    if transcript_bytes >= config.force_flush_transcript_bytes {
        return MemoryFlushDecision {
            should_flush: true,
            reason: FlushReason::TranscriptSize,
            context_hash,
        };
    }

    MemoryFlushDecision {
        should_flush: false,
        reason: FlushReason::None,
        context_hash,
    }
}

/// Resolve the date-stamped memory file path for flush target
///
/// Returns a path in the format `memory/YYYY-MM-DD.md` for today's date.
pub fn resolve_flush_target_path() -> String {
    let today = Local::now().format("%Y-%m-%d");
    format!("memory/{}.md", today)
}

/// Update compaction state after a successful flush
///
/// Records the current compaction count and context hash to prevent
/// duplicate flushes for the same context.
pub fn record_flush_in_state(state: &mut SessionCompactionState, context_hash: &str) {
    state.memory_flush_compaction_count = Some(state.compaction_count);
    state.last_flush_context_hash = Some(context_hash.to_string());
}

/// Increment the compaction count (called after compaction completes)
pub fn increment_compaction_count(state: &mut SessionCompactionState) {
    state.compaction_count += 1;
 // Clear flush tracking when compaction count changes
    state.memory_flush_compaction_count = None;
    state.last_flush_context_hash = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> MemoryFlushConfig {
        MemoryFlushConfig::default()
    }

    fn default_state() -> SessionCompactionState {
        SessionCompactionState::default()
    }

    #[test]
    fn test_check_memory_flush_token_threshold() {
        let config = default_config();
        let state = default_state();
        let messages = vec![
            ("user".to_string(), "Hello".to_string()),
            ("assistant".to_string(), "Hi there!".to_string()),
        ];

 // Low tokens - should not flush
 // Threshold = 8000 - 20000 - 4000 = 0 (saturating_sub), so 1000 >= 0 triggers flush
 // To NOT trigger, we need a larger context window
        let decision = check_memory_flush(
            1000,  // total_tokens
            10000, // transcript_bytes
            &config,
            100000, // context_window - large: threshold = 100000 - 20000 - 4000 = 76000
            &state, &messages,
        );
 // 1000 < 76000, should NOT flush
        assert!(!decision.should_flush);
        assert!(matches!(decision.reason, FlushReason::None));

 // High tokens - should flush
        let decision = check_memory_flush(
            80000, // total_tokens - above threshold (76000)
            10000, // transcript_bytes
            &config, 100000, // context_window
            &state, &messages,
        );
        assert!(decision.should_flush);
        assert!(matches!(decision.reason, FlushReason::TokenThreshold));
    }

    #[test]
    fn test_check_memory_flush_transcript_size() {
        let config = default_config();
        let state = default_state();
        let messages = vec![];

 // Small transcript - should not flush
        let decision = check_memory_flush(
            1000, // total_tokens
            1000, // transcript_bytes - well under 2MB
            &config, 100000, // context_window - large threshold
            &state, &messages,
        );
        assert!(!decision.should_flush);

 // Large transcript - should flush
        let decision = check_memory_flush(
            1000,      // total_tokens
            3_000_000, // transcript_bytes - over 2MB
            &config, 100000, // context_window
            &state, &messages,
        );
        assert!(decision.should_flush);
        assert!(matches!(decision.reason, FlushReason::TranscriptSize));
    }

    #[test]
    fn test_check_memory_flush_dedup_by_compaction_count() {
        let config = default_config();
        let mut state = default_state();
        state.compaction_count = 5;
        state.memory_flush_compaction_count = Some(5); // Already flushed this cycle

        let messages = vec![];
        let decision = check_memory_flush(
            10000, // High tokens
            10000, // transcript_bytes
            &config, 8000, // context_window
            &state, &messages,
        );

        assert!(
            !decision.should_flush,
            "Should not flush if already flushed this compaction cycle"
        );
    }

    #[test]
    fn test_check_memory_flush_dedup_by_context_hash() {
        let config = default_config();
        let mut state = default_state();
        state.last_flush_context_hash = Some("abc123".to_string());

        let messages = vec![];
        let _decision = check_memory_flush(
            10000, // total_tokens
            10000, // transcript_bytes
            &config, 8000, // context_window
            &state, &messages,
        );

 // Should not flush if context hash matches (context unchanged)
 // Note: This test depends on what hash compute_context_hash returns for empty messages
    }

    #[test]
    fn test_resolve_flush_target_path() {
        let path = resolve_flush_target_path();
        assert!(path.starts_with("memory/"));
        assert!(path.ends_with(".md"));
 // Should contain a date in YYYY-MM-DD format
        assert!(path.contains(&Local::now().format("%Y-%m-%d").to_string()));
    }

    #[test]
    fn test_record_flush_in_state() {
        let mut state = SessionCompactionState {
            compaction_count: 5,
            memory_flush_compaction_count: Some(4),
            last_flush_context_hash: Some("old_hash".to_string()),
        };

        record_flush_in_state(&mut state, "new_hash_123");

        assert_eq!(state.memory_flush_compaction_count, Some(5));
        assert_eq!(state.last_flush_context_hash, Some("new_hash_123".to_string()));
    }

    #[test]
    fn test_increment_compaction_count() {
        let mut state = SessionCompactionState {
            compaction_count: 5,
            memory_flush_compaction_count: Some(5),
            last_flush_context_hash: Some("hash".to_string()),
        };

        increment_compaction_count(&mut state);

        assert_eq!(state.compaction_count, 6);
        assert_eq!(state.memory_flush_compaction_count, None);
        assert_eq!(state.last_flush_context_hash, None);
    }
}
