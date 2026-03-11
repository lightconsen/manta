//! Memory system for Manta
//!
//! Provides persistent storage for conversations, messages, and memories
//! with support for semantic search using embeddings.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

pub mod db;
pub mod dual;
pub mod session_search;
pub mod sqlite;

pub use db::{DatabaseStore, DbStats, QueryBuilder};
pub use dual::{DualMemory, DualMemoryType};
pub use session_search::{SearchResult, SessionSearch, SessionSearchQuery};
pub use sqlite::SqliteMemoryStore;

/// Unique identifier for a memory entry
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemoryId(pub String);

impl MemoryId {
    /// Create a new memory ID
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Generate a new random ID
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for MemoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A memory entry stored in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    /// Unique identifier
    pub id: MemoryId,
    /// User ID who owns this memory
    pub user_id: String,
    /// Optional conversation ID
    pub conversation_id: Option<String>,
    /// Memory content
    pub content: String,
    /// Memory type (e.g., "fact", "preference", "context")
    pub memory_type: String,
    /// Optional embedding vector for semantic search
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// When the memory was created
    pub created_at: SystemTime,
    /// When the memory expires (None = never)
    pub expires_at: Option<SystemTime>,
    /// Additional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl Memory {
    /// Create a new memory entry
    pub fn new(
        user_id: impl Into<String>,
        content: impl Into<String>,
        memory_type: impl Into<String>,
    ) -> Self {
        Self {
            id: MemoryId::generate(),
            user_id: user_id.into(),
            conversation_id: None,
            content: content.into(),
            memory_type: memory_type.into(),
            embedding: None,
            created_at: SystemTime::now(),
            expires_at: None,
            metadata: None,
        }
    }

    /// Set the conversation ID
    pub fn with_conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    /// Set the embedding vector
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set expiration time (TTL in seconds)
    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.expires_at = Some(
            SystemTime::now() + std::time::Duration::from_secs(ttl_seconds),
        );
        self
    }

    /// Set metadata
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Check if the memory has expired
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .map(|exp| SystemTime::now() > exp)
            .unwrap_or(false)
    }
}

/// Query options for searching memories
#[derive(Debug, Clone, Default)]
pub struct MemoryQuery {
    /// Filter by user ID
    pub user_id: Option<String>,
    /// Filter by conversation ID
    pub conversation_id: Option<String>,
    /// Filter by memory type
    pub memory_type: Option<String>,
    /// Search query for content matching
    pub content_query: Option<String>,
    /// Embedding for semantic search
    pub embedding: Option<Vec<f32>>,
    /// Maximum number of results
    pub limit: usize,
    /// Offset for pagination
    pub offset: usize,
    /// Include expired memories
    pub include_expired: bool,
}

impl MemoryQuery {
    /// Create a new query
    pub fn new() -> Self {
        Self {
            limit: 10,
            ..Default::default()
        }
    }

    /// Filter by user ID
    pub fn for_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Filter by conversation ID
    pub fn for_conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    /// Filter by memory type
    pub fn of_type(mut self, memory_type: impl Into<String>) -> Self {
        self.memory_type = Some(memory_type.into());
        self
    }

    /// Search by content
    pub fn with_content(mut self, query: impl Into<String>) -> Self {
        self.content_query = Some(query.into());
        self
    }

    /// Search by semantic similarity
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Set result limit
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Include expired memories
    pub fn include_expired(mut self) -> Self {
        self.include_expired = true;
        self
    }
}

/// Statistics about the memory store
#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    /// Total number of memories
    pub total_count: usize,
    /// Number of memories per type
    pub count_by_type: std::collections::HashMap<String, usize>,
    /// Number of expired memories
    pub expired_count: usize,
}

/// Trait for memory storage backends
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Store a new memory
    async fn store(&self, memory: Memory) -> crate::Result<MemoryId>;

    /// Retrieve a memory by ID
    async fn get(&self, id: &MemoryId) -> crate::Result<Option<Memory>>;

    /// Update an existing memory
    async fn update(&self, memory: Memory) -> crate::Result<()>;

    /// Delete a memory by ID
    async fn delete(&self, id: &MemoryId) -> crate::Result<bool>;

    /// Search memories based on query
    async fn search(&self, query: MemoryQuery) -> crate::Result<Vec<Memory>>;

    /// Delete expired memories
    async fn cleanup_expired(&self) -> crate::Result<usize>;

    /// Get statistics
    async fn stats(&self) -> crate::Result<MemoryStats>;

    /// Close the store (clean up resources)
    async fn close(&self) -> crate::Result<()>;
}

/// Calculate cosine similarity between two vectors
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot_product / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_id() {
        let id = MemoryId::new("test_id");
        assert_eq!(id.0, "test_id");
    }

    #[test]
    fn test_memory_creation() {
        let memory = Memory::new("user1", "Hello world", "fact")
            .with_conversation("conv1")
            .with_ttl(3600);

        assert_eq!(memory.user_id, "user1");
        assert_eq!(memory.content, "Hello world");
        assert_eq!(memory.memory_type, "fact");
        assert_eq!(memory.conversation_id, Some("conv1".to_string()));
        assert!(memory.expires_at.is_some());
        assert!(!memory.is_expired());
    }

    #[test]
    fn test_memory_query() {
        let query = MemoryQuery::new()
            .for_user("user1")
            .of_type("fact")
            .limit(5);

        assert_eq!(query.user_id, Some("user1".to_string()));
        assert_eq!(query.memory_type, Some("fact".to_string()));
        assert_eq!(query.limit, 5);
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let c = vec![1.0, 0.0, 0.0];

        // Orthogonal vectors have 0 similarity
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 0.001);

        // Same vectors have 1.0 similarity
        assert!((cosine_similarity(&a, &c) - 1.0).abs() < 0.001);

        // Empty vectors return 0
        assert_eq!(cosine_similarity(&[], &[]), 0.0);

        // Different length vectors return 0
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }
}
