//! Memory Event Log System
//!
//! Provides JSONL-based event logging for memory operations.
//! Events are appended to `{workspace}/memory/.dreams/events.jsonl`.
//!
//! Event types:
//! - Recall: when memories are recalled into context
//! - Promotion: when memories are promoted between tiers
//! - Compact: when session compaction occurs
//! - Dream: when a dreaming cycle completes

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Event log relative path within workspace.
pub const MEMORY_EVENT_LOG_RELATIVE_PATH: &str = "memory/.dreams/events.jsonl";

/// All memory event variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MemoryEvent {
    /// Memory recalled and injected into session context.
    RecallRecorded {
        timestamp: u64,
        session_key: String,
        recall_id: String,
        source: String,
        content_summary: String,
    },
    /// Memory promoted to a higher tier.
    PromotionApplied {
        timestamp: u64,
        session_key: String,
        promotion_id: String,
        from_level: String,
        to_level: String,
        reason: String,
    },
    /// Session compacted / summarized.
    CompactCompleted {
        timestamp: u64,
        session_key: String,
        compact_id: String,
        messages_processed: u32,
        memories_created: u32,
    },
    /// Dreaming cycle completed.
    DreamCompleted {
        timestamp: u64,
        dream_id: String,
        phase: DreamPhase,
        summary: String,
        memories_processed: u32,
        memories_created: u32,
    },
}

/// Dream phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamPhase {
    Light,
    Deep,
    Rem,
}

impl std::fmt::Display for DreamPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DreamPhase::Light => write!(f, "light"),
            DreamPhase::Deep => write!(f, "deep"),
            DreamPhase::Rem => write!(f, "rem"),
        }
    }
}

/// Builder for memory events.
pub struct MemoryEventBuilder {
    timestamp: u64,
}

impl MemoryEventBuilder {
    pub fn new() -> Self {
        Self {
            timestamp: chrono::Utc::now().timestamp() as u64,
        }
    }

    pub fn recall(
        self,
        session_key: impl Into<String>,
        recall_id: impl Into<String>,
        source: impl Into<String>,
        content_summary: impl Into<String>,
    ) -> MemoryEvent {
        MemoryEvent::RecallRecorded {
            timestamp: self.timestamp,
            session_key: session_key.into(),
            recall_id: recall_id.into(),
            source: source.into(),
            content_summary: content_summary.into(),
        }
    }

    pub fn promotion(
        self,
        session_key: impl Into<String>,
        promotion_id: impl Into<String>,
        from_level: impl Into<String>,
        to_level: impl Into<String>,
        reason: impl Into<String>,
    ) -> MemoryEvent {
        MemoryEvent::PromotionApplied {
            timestamp: self.timestamp,
            session_key: session_key.into(),
            promotion_id: promotion_id.into(),
            from_level: from_level.into(),
            to_level: to_level.into(),
            reason: reason.into(),
        }
    }

    pub fn compact(
        self,
        session_key: impl Into<String>,
        compact_id: impl Into<String>,
        messages_processed: u32,
        memories_created: u32,
    ) -> MemoryEvent {
        MemoryEvent::CompactCompleted {
            timestamp: self.timestamp,
            session_key: session_key.into(),
            compact_id: compact_id.into(),
            messages_processed,
            memories_created,
        }
    }

    pub fn dream(
        self,
        dream_id: impl Into<String>,
        phase: DreamPhase,
        summary: impl Into<String>,
        memories_processed: u32,
        memories_created: u32,
    ) -> MemoryEvent {
        MemoryEvent::DreamCompleted {
            timestamp: self.timestamp,
            dream_id: dream_id.into(),
            phase,
            summary: summary.into(),
            memories_processed,
            memories_created,
        }
    }
}

impl Default for MemoryEventBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Append a memory event to the JSONL log.
pub async fn append_memory_event(
    workspace_dir: impl AsRef<std::path::Path>,
    event: &MemoryEvent,
) -> crate::Result<()> {
    let path = workspace_dir.as_ref().join(MEMORY_EVENT_LOG_RELATIVE_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to create event log directory: {:?}", parent),
                details: e.to_string(),
            })?;
    }

    let line = serde_json::to_string(event).map_err(|e| crate::error::SyscityError::Storage {
        context: "Failed to serialize memory event".to_string(),
        details: e.to_string(),
    })?;

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: format!("Failed to open event log: {:?}", path),
            details: e.to_string(),
        })?;

    file.write_all(line.as_bytes())
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: format!("Failed to write event log: {:?}", path),
            details: e.to_string(),
        })?;
    file.write_all(b"\n")
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: format!("Failed to write newline to event log: {:?}", path),
            details: e.to_string(),
        })?;

    Ok(())
}

/// Read all memory events from the JSONL log.
pub async fn read_memory_events(
    workspace_dir: impl AsRef<std::path::Path>,
) -> crate::Result<Vec<MemoryEvent>> {
    let path = workspace_dir.as_ref().join(MEMORY_EVENT_LOG_RELATIVE_PATH);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content =
        fs::read_to_string(&path)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to read event log: {:?}", path),
                details: e.to_string(),
            })?;

    let mut events = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<MemoryEvent>(line) {
            Ok(event) => events.push(event),
            Err(e) => {
                tracing::warn!("Skipping malformed memory event line: {}", e);
            }
        }
    }

    Ok(events)
}

/// Event log service wrapper.
#[derive(Debug, Clone)]
pub struct MemoryEventLog {
    workspace_dir: PathBuf,
}

impl MemoryEventLog {
    /// Create a new event log for the given workspace.
    pub fn new(workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
        }
    }

    /// Append an event.
    pub async fn append(&self, event: &MemoryEvent) -> crate::Result<()> {
        append_memory_event(&self.workspace_dir, event).await
    }

    /// Read all events.
    pub async fn read_all(&self) -> crate::Result<Vec<MemoryEvent>> {
        read_memory_events(&self.workspace_dir).await
    }

    /// Read events filtered by type.
    pub async fn read_by_type(&self, event_type: &str) -> crate::Result<Vec<MemoryEvent>> {
        let all = self.read_all().await?;
        Ok(all
            .into_iter()
            .filter(|e| e.event_type() == event_type)
            .collect())
    }
}

impl MemoryEvent {
    /// Return the event type discriminator.
    pub fn event_type(&self) -> &'static str {
        match self {
            MemoryEvent::RecallRecorded { .. } => "recall",
            MemoryEvent::PromotionApplied { .. } => "promotion",
            MemoryEvent::CompactCompleted { .. } => "compact",
            MemoryEvent::DreamCompleted { .. } => "dream",
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn test_event_log_roundtrip() {
        let dir = tempdir().unwrap();
        let log = MemoryEventLog::new(dir.path());

        let event1 = MemoryEventBuilder::new().recall(
            "session:1",
            "r1",
            "hybrid_search",
            "User likes coffee",
        );
        let event2 = MemoryEventBuilder::new().promotion(
            "session:1",
            "p1",
            "short_term",
            "long_term",
            "High importance",
        );

        log.append(&event1).await.unwrap();
        log.append(&event2).await.unwrap();

        let events = log.read_all().await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type(), "recall");
        assert_eq!(events[1].event_type(), "promotion");
    }

    #[test]
    fn test_dream_phase_display() {
        assert_eq!(format!("{}", DreamPhase::Light), "light");
        assert_eq!(format!("{}", DreamPhase::Deep), "deep");
        assert_eq!(format!("{}", DreamPhase::Rem), "rem");
    }
}
