//! Trajectory Log
//!
//! Captures the execution trace of an agent turn: tool calls,
//! reasoning steps, provider latencies, and other observability data.
//!
//! Design matches OpenClaw's `src/trajectory/`.

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// A single entry in the trajectory log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum TrajectoryEntry {
    /// The agent started processing.
    Start {
        timestamp: SystemTime,
        session_id: String,
        agent_id: String,
    },
    /// A tool was invoked.
    ToolCall {
        timestamp: SystemTime,
        name: String,
        arguments: serde_json::Value,
    },
    /// A tool returned a result.
    ToolResult {
        timestamp: SystemTime,
        name: String,
        result: serde_json::Value,
        duration_ms: u64,
    },
    /// The LLM provider was called.
    LlmCall {
        timestamp: SystemTime,
        provider: String,
        model: String,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        duration_ms: u64,
    },
    /// A reasoning or planning step.
    Reasoning {
        timestamp: SystemTime,
        step: String,
        detail: String,
    },
    /// The agent finished.
    Finish {
        timestamp: SystemTime,
        output: String,
    },
    /// An error occurred.
    Error {
        timestamp: SystemTime,
        message: String,
    },
}

/// The full trajectory for a single agent turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrajectoryLog {
    pub entries: Vec<TrajectoryEntry>,
}

impl TrajectoryLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: TrajectoryEntry) {
        self.entries.push(entry);
    }

    pub fn tool_calls(&self) -> Vec<&TrajectoryEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e, TrajectoryEntry::ToolCall { .. }))
            .collect()
    }

    pub fn llm_calls(&self) -> Vec<&TrajectoryEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e, TrajectoryEntry::LlmCall { .. }))
            .collect()
    }

    pub fn total_duration_ms(&self) -> u64 {
        let start = self.entries.iter().find_map(|e| match e {
            TrajectoryEntry::Start { timestamp, .. } => Some(*timestamp),
            _ => None,
        });
        let end = self.entries.iter().find_map(|e| match e {
            TrajectoryEntry::Finish { timestamp, .. } => Some(*timestamp),
            _ => None,
        });
        match (start, end) {
            (Some(s), Some(e)) => e.duration_since(s).unwrap_or_default().as_millis() as u64,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trajectory_push() {
        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::Start {
            timestamp: SystemTime::now(),
            session_id: "s1".to_string(),
            agent_id: "a1".to_string(),
        });
        assert_eq!(log.entries.len(), 1);
    }

    #[test]
    fn test_tool_calls_filter() {
        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::Start {
            timestamp: SystemTime::now(),
            session_id: "s1".to_string(),
            agent_id: "a1".to_string(),
        });
        log.push(TrajectoryEntry::ToolCall {
            timestamp: SystemTime::now(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "rust"}),
        });
        assert_eq!(log.tool_calls().len(), 1);
    }

    #[test]
    fn test_llm_calls_filter() {
        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::LlmCall {
            timestamp: SystemTime::now(),
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            input_tokens: Some(10),
            output_tokens: Some(20),
            duration_ms: 500,
        });
        log.push(TrajectoryEntry::ToolCall {
            timestamp: SystemTime::now(),
            name: "search".to_string(),
            arguments: serde_json::json!({}),
        });
        assert_eq!(log.llm_calls().len(), 1);
    }

    #[test]
    fn test_total_duration_ms() {
        let start = SystemTime::now();
        let end = start + std::time::Duration::from_millis(250);

        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::Start {
            timestamp: start,
            session_id: "s1".to_string(),
            agent_id: "a1".to_string(),
        });
        log.push(TrajectoryEntry::Finish {
            timestamp: end,
            output: "done".to_string(),
        });

        assert_eq!(log.total_duration_ms(), 250);
    }

    #[test]
    fn test_total_duration_no_start() {
        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::Finish {
            timestamp: SystemTime::now(),
            output: "done".to_string(),
        });
        assert_eq!(log.total_duration_ms(), 0);
    }

    #[test]
    fn test_trajectory_entry_serialization() {
        let entry = TrajectoryEntry::Error {
            timestamp: SystemTime::now(),
            message: "oops".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("oops"));
    }
}
