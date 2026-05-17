//! Export format definitions and data structures
//!
//! Defines the JSON/JSONL serialization formats for memories and conversations.
//! These formats are designed to be:
//! - Human-readable (Markdown for conversations)
//! - Machine-parseable (JSON/JSONL)
//! - Compatible with OpenClaw-style exports for interoperability

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Export format options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// Markdown format (human-readable conversation transcript)
    Markdown,
    /// JSON format (structured, single file)
    Json,
    /// JSON Lines format (one record per line, streaming-friendly)
    Jsonl,
}

impl std::str::FromStr for ExportFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "markdown" | "md" => Ok(ExportFormat::Markdown),
            "json" => Ok(ExportFormat::Json),
            "jsonl" | "jsonlines" => Ok(ExportFormat::Jsonl),
            _ => Err(format!("Unknown export format: {}", s)),
        }
    }
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportFormat::Markdown => write!(f, "markdown"),
            ExportFormat::Json => write!(f, "json"),
            ExportFormat::Jsonl => write!(f, "jsonl"),
        }
    }
}

impl ExportFormat {
    /// Get the file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::Json => "json",
            ExportFormat::Jsonl => "jsonl",
        }
    }
}

/// JSONL representation of a chat message
/// Compatible with OpenClaw-style transcript files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonLineMessage {
    /// Unique message identifier
    pub id: String,
    /// Conversation identifier
    pub conversation_id: String,
    /// User identifier
    pub user_id: String,
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp (ISO 8601 format)
    pub timestamp: String,
    /// Optional metadata (tool calls, tokens, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl JsonLineMessage {
    /// Create from a ChatMessage
    pub fn from_chat_message(msg: &crate::memory::ChatMessage) -> Self {
        Self {
            id: msg.id.clone(),
            conversation_id: msg.conversation_id.clone(),
            user_id: msg.user_id.clone(),
            role: msg.role.clone(),
            content: msg.content.clone(),
            timestamp: humantime_timestamp(msg.created_at),
            metadata: msg.metadata.clone(),
        }
    }
}

/// JSONL representation of a memory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonLineMemory {
    /// Unique memory identifier
    pub id: String,
    /// User identifier
    pub user_id: String,
    /// Optional conversation identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Memory content
    pub content: String,
    /// Memory type (fact, preference, semantic, etc.)
    pub memory_type: String,
    /// Creation timestamp (ISO 8601)
    pub created_at: String,
    /// Expiration timestamp (ISO 8601), if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Importance score [0.0, 1.0]
    pub importance_score: f32,
    /// Source of the memory (agent, user, compaction)
    pub source: String,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// Embedding vector (optional, excluded by default for size)
    #[serde(skip_serializing)]
    pub embedding: Option<Vec<f32>>,
}

impl JsonLineMemory {
    /// Create from a Memory, optionally including the embedding
    pub fn from_memory(memory: &crate::memory::Memory, include_embedding: bool) -> Self {
        Self {
            id: memory.id.to_string(),
            user_id: memory.user_id.clone(),
            conversation_id: memory.conversation_id.clone(),
            content: memory.content.clone(),
            memory_type: memory.memory_type.clone(),
            created_at: humantime_timestamp(memory.created_at),
            expires_at: memory.expires_at.map(humantime_timestamp),
            importance_score: memory.importance_score,
            source: memory.source.clone(),
            metadata: memory.metadata.clone(),
            embedding: if include_embedding {
                memory.embedding.clone()
            } else {
                None
            },
        }
    }
}

/// Conversation export in JSON format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationExport {
    /// Export metadata
    pub meta: ExportMeta,
    /// Conversation messages
    pub messages: Vec<JsonLineMessage>,
}

/// Memory export in JSON format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryExport {
    /// Export metadata
    pub meta: ExportMeta,
    /// Memory entries
    pub memories: Vec<JsonLineMemory>,
}

/// Combined export (conversations + memories)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullExport {
    /// Export metadata
    pub meta: ExportMeta,
    /// Conversations mapped by ID
    pub conversations: std::collections::HashMap<String, Vec<JsonLineMessage>>,
    /// Memory entries
    pub memories: Vec<JsonLineMemory>,
}

/// Export metadata header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportMeta {
    /// Export version
    pub version: String,
    /// Export timestamp
    pub exported_at: String,
    /// Source application
    pub source: String,
    /// Export format version
    pub format_version: u32,
}

impl ExportMeta {
    /// Create new export metadata
    pub fn new() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            exported_at: chrono::Utc::now().to_rfc3339(),
            source: "manta".to_string(),
            format_version: 1,
        }
    }
}

impl Default for ExportMeta {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert SystemTime to ISO 8601 string
fn humantime_timestamp(time: SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Utc> = time.into();
    datetime.to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_export_format_from_str() {
        assert_eq!(ExportFormat::from_str("markdown").unwrap(), ExportFormat::Markdown);
        assert_eq!(ExportFormat::from_str("md").unwrap(), ExportFormat::Markdown);
        assert_eq!(ExportFormat::from_str("json").unwrap(), ExportFormat::Json);
        assert_eq!(ExportFormat::from_str("jsonl").unwrap(), ExportFormat::Jsonl);
        assert_eq!(ExportFormat::from_str("jsonlines").unwrap(), ExportFormat::Jsonl);
        assert!(ExportFormat::from_str("unknown").is_err());
    }

    #[test]
    fn test_export_format_from_str_case_insensitive() {
        assert_eq!(ExportFormat::from_str("Markdown").unwrap(), ExportFormat::Markdown);
        assert_eq!(ExportFormat::from_str("JSON").unwrap(), ExportFormat::Json);
        assert_eq!(ExportFormat::from_str("JSONL").unwrap(), ExportFormat::Jsonl);
    }

    #[test]
    fn test_export_format_extension() {
        assert_eq!(ExportFormat::Markdown.extension(), "md");
        assert_eq!(ExportFormat::Json.extension(), "json");
        assert_eq!(ExportFormat::Jsonl.extension(), "jsonl");
    }

    #[test]
    fn test_export_format_display() {
        assert_eq!(ExportFormat::Markdown.to_string(), "markdown");
        assert_eq!(ExportFormat::Json.to_string(), "json");
        assert_eq!(ExportFormat::Jsonl.to_string(), "jsonl");
    }

    #[test]
    fn test_export_meta() {
        let meta = ExportMeta::new();
        assert_eq!(meta.source, "manta");
        assert_eq!(meta.format_version, 1);
        assert!(!meta.exported_at.is_empty());
        assert!(!meta.version.is_empty());
    }

    #[test]
    fn test_export_meta_default() {
        let meta: ExportMeta = Default::default();
        assert_eq!(meta.source, "manta");
        assert_eq!(meta.format_version, 1);
    }

    #[test]
    fn test_export_meta_serde() {
        let meta = ExportMeta::new();
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("manta"));
        let restored: ExportMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.source, "manta");
        assert_eq!(restored.format_version, 1);
    }

    #[test]
    fn test_humantime_timestamp() {
        let now = SystemTime::now();
        let ts = humantime_timestamp(now);
        assert!(!ts.is_empty());
        assert!(ts.contains('T') || ts.contains('+'));
    }

    #[test]
    fn test_json_line_message_from_chat_message() {
        let msg = crate::memory::ChatMessage::new("conv1", "user1", "user", "hello");
        let json_msg = JsonLineMessage::from_chat_message(&msg);
        assert_eq!(json_msg.conversation_id, "conv1");
        assert_eq!(json_msg.user_id, "user1");
        assert_eq!(json_msg.role, "user");
        assert_eq!(json_msg.content, "hello");
        assert_eq!(json_msg.id, msg.id);
        assert!(json_msg.metadata.is_none());
    }

    #[test]
    fn test_json_line_message_serde() {
        let msg = JsonLineMessage {
            id: "1".to_string(),
            conversation_id: "conv1".to_string(),
            user_id: "user1".to_string(),
            role: "assistant".to_string(),
            content: "hi".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            metadata: Some(serde_json::json!({"tokens": 10})),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("assistant"));
        let restored: JsonLineMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.content, "hi");
    }

    #[test]
    fn test_json_line_memory_from_memory() {
        let memory = crate::memory::Memory::new("user1", "hello world", "fact");
        let json_mem = JsonLineMemory::from_memory(&memory, false);
        assert_eq!(json_mem.user_id, "user1");
        assert_eq!(json_mem.content, "hello world");
        assert_eq!(json_mem.memory_type, "fact");
        assert_eq!(json_mem.importance_score, 0.5);
        assert_eq!(json_mem.source, "agent");
        assert!(json_mem.embedding.is_none());
    }

    #[test]
    fn test_json_line_memory_with_embedding() {
        let memory = crate::memory::Memory::new("user1", "hello", "fact")
            .with_embedding(vec![0.1, 0.2, 0.3]);
        let json_mem = JsonLineMemory::from_memory(&memory, true);
        assert!(json_mem.embedding.is_some());
        assert_eq!(json_mem.embedding.unwrap(), vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn test_json_line_memory_without_embedding() {
        let memory = crate::memory::Memory::new("user1", "hello", "fact")
            .with_embedding(vec![0.1, 0.2, 0.3]);
        let json_mem = JsonLineMemory::from_memory(&memory, false);
        assert!(json_mem.embedding.is_none());
    }

    #[test]
    fn test_conversation_export_serde() {
        let export = ConversationExport {
            meta: ExportMeta::new(),
            messages: vec![],
        };
        let json = serde_json::to_string(&export).unwrap();
        assert!(json.contains("manta"));
        let restored: ConversationExport = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.messages.len(), 0);
    }

    #[test]
    fn test_memory_export_serde() {
        let export = MemoryExport {
            meta: ExportMeta::new(),
            memories: vec![],
        };
        let json = serde_json::to_string(&export).unwrap();
        assert!(json.contains("manta"));
    }

    #[test]
    fn test_full_export_serde() {
        let export = FullExport {
            meta: ExportMeta::new(),
            conversations: std::collections::HashMap::new(),
            memories: vec![],
        };
        let json = serde_json::to_string(&export).unwrap();
        assert!(json.contains("manta"));
        let restored: FullExport = serde_json::from_str(&json).unwrap();
        assert!(restored.conversations.is_empty());
    }

    #[test]
    fn test_json_line_memory_serde_skip_embedding() {
        let mem = JsonLineMemory {
            id: "1".to_string(),
            user_id: "u1".to_string(),
            conversation_id: None,
            content: "test".to_string(),
            memory_type: "fact".to_string(),
            created_at: "2024-01-01T00:00:00Z".to_string(),
            expires_at: None,
            importance_score: 0.5,
            source: "agent".to_string(),
            metadata: None,
            embedding: Some(vec![1.0, 2.0]),
        };
        let json = serde_json::to_string(&mem).unwrap();
        // Embedding should be skipped in serialization
        assert!(!json.contains("embedding"));
    }
}
