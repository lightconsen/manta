//! Turn observability record types.
//!
//! One `TurnRecord` per agent turn, persisted as a JSON file under
//! `~/.syscity/turns/YYYY-MM-DD/<turn_id>.json`. Large free-text fields are
//! truncated to [`MAX_FIELD_BYTES`] at persistence time via
//! [`TurnRecord::finalize`].

use serde::{Deserialize, Serialize};

/// Maximum bytes kept for free-text fields (args, results, previews, errors).
pub const MAX_FIELD_BYTES: usize = 4096;

/// Current turn record schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndState {
    Complete,
    Error,
    Aborted,
}

/// Token usage for a single LLM call or a whole turn.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ObservedUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default)]
    pub cache_read_tokens: u32,
    #[serde(default)]
    pub cache_creation_tokens: u32,
}

/// Where a failure originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSource {
    Llm,
    Tool,
    Internal,
}

/// A recorded failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedError {
    pub source: ErrorSource,
    pub message: String,
}

/// One round of the LLM call loop within a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRoundRecord {
    pub round: u32,
    pub provider: String,
    pub model: String,
    pub started_at: String,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub usage: Option<ObservedUsage>,
    pub finish_reason: Option<String>,
    pub error: Option<String>,
}

/// One tool invocation within a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedToolCall {
    pub round: u32,
    pub name: String,
    pub args: String,
    pub result: String,
    pub success: bool,
    pub duration_ms: u64,
    pub error: Option<String>,
}

/// A complete per-turn observability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub schema_version: u32,
    pub turn_id: String,
    pub session_id: Option<String>,
    pub conversation_id: String,
    /// Stable agent identifier that handled this turn (empty for ephemeral
    /// subagents). Backward-compatible: old records without the field default
    /// to an empty string.
    #[serde(default)]
    pub agent_id: String,
    pub thread_id: String,
    pub turn_index: u32,
    pub state: TurnEndState,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub model: String,
    pub user_message_preview: String,
    pub assistant_text_preview: String,
    pub reasoning_preview: String,
    pub queue_wait_ms: Option<u64>,
    pub cache_hit: bool,
    pub error: Option<ObservedError>,
    pub usage: ObservedUsage,
    pub llm_rounds: Vec<LlmRoundRecord>,
    pub tool_calls: Vec<ObservedToolCall>,
}

/// Truncate `s` to at most `MAX_FIELD_BYTES` bytes on a char boundary.
pub fn truncate_field(s: &mut String) {
    if s.len() <= MAX_FIELD_BYTES {
        return;
    }
    let mut end = MAX_FIELD_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

impl TurnRecord {
    /// Apply field-size limits before persistence.
    pub fn finalize(&mut self) {
        truncate_field(&mut self.user_message_preview);
        truncate_field(&mut self.assistant_text_preview);
        truncate_field(&mut self.reasoning_preview);
        if let Some(err) = &mut self.error {
            truncate_field(&mut err.message);
        }
        for round in &mut self.llm_rounds {
            if let Some(e) = &mut round.error {
                truncate_field(e);
            }
        }
        for call in &mut self.tool_calls {
            truncate_field(&mut call.args);
            truncate_field(&mut call.result);
            if let Some(e) = &mut call.error {
                truncate_field(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_field_multibyte_char_boundary() {
        // Each '中' is 3 bytes; 4096 bytes lands mid-char and must back off.
        let mut s = "中".repeat(2000);
        truncate_field(&mut s);
        assert!(s.len() <= MAX_FIELD_BYTES);
        assert!(s.is_char_boundary(s.len()));
        assert_eq!(s.len(), 4095); // 1365 chars * 3 bytes
    }

    #[test]
    fn truncate_field_short_string_untouched() {
        let mut s = "hello".to_string();
        truncate_field(&mut s);
        assert_eq!(s, "hello");
    }

    #[test]
    fn record_serde_round_trip() {
        let rec = TurnRecord {
            schema_version: SCHEMA_VERSION,
            turn_id: "t1".into(),
            session_id: Some("s1".into()),
            conversation_id: "c1".into(),
            agent_id: "main".into(),
            thread_id: "main".into(),
            turn_index: 0,
            state: TurnEndState::Complete,
            started_at: "2026-08-14T10:00:00+08:00".into(),
            finished_at: "2026-08-14T10:00:01+08:00".into(),
            duration_ms: 1000,
            ttft_ms: Some(120),
            model: "m".into(),
            user_message_preview: "u".into(),
            assistant_text_preview: "a".into(),
            reasoning_preview: String::new(),
            queue_wait_ms: Some(5),
            cache_hit: false,
            error: None,
            usage: ObservedUsage::default(),
            llm_rounds: vec![],
            tool_calls: vec![],
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: TurnRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.turn_id, "t1");
        assert_eq!(back.state, TurnEndState::Complete);
    }
}
