//! Storage adapter for Manta
//!
//! This module provides storage abstractions and implementations.

use crate::core::models::{Entity, Id};
use crate::error::MantaError;
use async_trait::async_trait;
use sqlx::Row;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use thiserror::Error;

// Re-export memory types for unified storage
pub use crate::memory::{
    ChatHistoryStore, ChatMessage, EmbeddedChunk, Memory, MemoryId, MemoryQuery, MemoryStats,
    MemoryStore, VectorStore, VectorStoreStats,
};

/// Errors that can occur during storage operations
#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Entity not found: {0}")]
    NotFound(Id),

    #[error("Storage is full")]
    Full,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Storage backend error: {0}")]
    Backend(String),
}

impl From<StorageError> for MantaError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::NotFound(id) => MantaError::NotFound {
                resource: format!("Entity {} not found", id),
            },
            StorageError::Full => MantaError::Validation("Storage is full".to_string()),
            StorageError::Io(e) => MantaError::Io(e),
            StorageError::Serialization(msg) => MantaError::Internal(format!("Serialization error: {}", msg)),
            StorageError::Backend(msg) => MantaError::Internal(format!("Storage backend: {}", msg)),
        }
    }
}

/// Storage trait for entity persistence
#[async_trait]
pub trait Storage: Send + Sync {
    /// Get an entity by ID
    async fn get(&self, id: Id) -> Result<Entity, StorageError>;

    /// List all entities, optionally filtered by status
    async fn list(&self) -> Result<Vec<Entity>, StorageError>;

    /// Create a new entity
    async fn create(&self, entity: &Entity) -> Result<(), StorageError>;

    /// Update an existing entity
    async fn update(&self, entity: &Entity) -> Result<(), StorageError>;

    /// Delete an entity
    async fn delete(&self, id: Id) -> Result<(), StorageError>;

    /// Count total entities
    async fn count(&self) -> Result<usize, StorageError>;

    /// Check if storage is healthy
    async fn health_check(&self) -> Result<(), StorageError>;
}

/// In-memory storage implementation
#[derive(Debug, Clone)]
pub struct InMemoryStorage {
    data: Arc<RwLock<HashMap<Id, Entity>>>,
    max_size: usize,
}

impl InMemoryStorage {
    /// Create a new in-memory storage with default capacity
    pub fn new() -> Self {
        Self::with_capacity(10_000)
    }

    /// Create a new in-memory storage with specified max size
    pub fn with_capacity(max_size: usize) -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::with_capacity(max_size.min(1000)))),
            max_size,
        }
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Storage for InMemoryStorage {
    async fn get(&self, id: Id) -> Result<Entity, StorageError> {
        let data = self.data.read().map_err(|_| {
            StorageError::Backend("Failed to acquire read lock".to_string())
        })?;

        data.get(&id)
            .cloned()
            .ok_or(StorageError::NotFound(id))
    }

    async fn list(&self) -> Result<Vec<Entity>, StorageError> {
        let data = self.data.read().map_err(|_| {
            StorageError::Backend("Failed to acquire read lock".to_string())
        })?;

        Ok(data.values().cloned().collect())
    }

    async fn create(&self, entity: &Entity) -> Result<(), StorageError> {
        let mut data = self.data.write().map_err(|_| {
            StorageError::Backend("Failed to acquire write lock".to_string())
        })?;

        if data.len() >= self.max_size {
            return Err(StorageError::Full);
        }

        data.insert(entity.id, entity.clone());
        Ok(())
    }

    async fn update(&self, entity: &Entity) -> Result<(), StorageError> {
        let mut data = self.data.write().map_err(|_| {
            StorageError::Backend("Failed to acquire write lock".to_string())
        })?;

        if !data.contains_key(&entity.id) {
            return Err(StorageError::NotFound(entity.id));
        }

        data.insert(entity.id, entity.clone());
        Ok(())
    }

    async fn delete(&self, id: Id) -> Result<(), StorageError> {
        let mut data = self.data.write().map_err(|_| {
            StorageError::Backend("Failed to acquire write lock".to_string())
        })?;

        data.remove(&id)
            .ok_or(StorageError::NotFound(id))
            .map(|_| ())
    }

    async fn count(&self) -> Result<usize, StorageError> {
        let data = self.data.read().map_err(|_| {
            StorageError::Backend("Failed to acquire read lock".to_string())
        })?;

        Ok(data.len())
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        // In-memory storage is always healthy unless we can't acquire the lock
        let _guard = self.data.read().map_err(|_| {
            StorageError::Backend("Storage lock poisoned".to_string())
        })?;
        drop(_guard);
        Ok(())
    }
}

/// File-based storage implementation
#[derive(Debug, Clone)]
pub struct FileStorage {
    base_path: PathBuf,
}

impl FileStorage {
    /// Create a new file storage at the given path
    pub fn new(base_path: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let base_path = base_path.into();
        std::fs::create_dir_all(&base_path)?;

        Ok(Self { base_path })
    }

    /// Get the path for a specific entity
    fn entity_path(&self, id: Id) -> PathBuf {
        self.base_path.join(format!("{}.json", id))
    }
}

#[async_trait]
impl Storage for FileStorage {
    async fn get(&self, id: Id) -> Result<Entity, StorageError> {
        let path = self.entity_path(id);

        if !path.exists() {
            return Err(StorageError::NotFound(id));
        }

        let content = tokio::fs::read_to_string(&path).await?;
        let entity: Entity = serde_json::from_str(&content)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        Ok(entity)
    }

    async fn list(&self) -> Result<Vec<Entity>, StorageError> {
        let mut entities = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let content = tokio::fs::read_to_string(&path).await?;
                if let Ok(entity) = serde_json::from_str::<Entity>(&content) {
                    entities.push(entity);
                }
            }
        }

        Ok(entities)
    }

    async fn create(&self, entity: &Entity) -> Result<(), StorageError> {
        let path = self.entity_path(entity.id);

        if path.exists() {
            return Err(StorageError::Backend(
                format!("Entity {} already exists", entity.id)
            ));
        }

        let content = serde_json::to_string_pretty(entity)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn update(&self, entity: &Entity) -> Result<(), StorageError> {
        let path = self.entity_path(entity.id);

        if !path.exists() {
            return Err(StorageError::NotFound(entity.id));
        }

        let content = serde_json::to_string_pretty(entity)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        tokio::fs::write(&path, content).await?;
        Ok(())
    }

    async fn delete(&self, id: Id) -> Result<(), StorageError> {
        let path = self.entity_path(id);

        if !path.exists() {
            return Err(StorageError::NotFound(id));
        }

        tokio::fs::remove_file(&path).await?;
        Ok(())
    }

    async fn count(&self) -> Result<usize, StorageError> {
        let mut count = 0;
        let mut entries = tokio::fs::read_dir(&self.base_path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                count += 1;
            }
        }

        Ok(count)
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        // Check if we can read the directory
        let _ = tokio::fs::read_dir(&self.base_path).await?;
        Ok(())
    }
}

/// SQLite-backed storage implementation
#[derive(Debug, Clone)]
pub struct SqliteStorage {
    pool: sqlx::SqlitePool,
}

impl SqliteStorage {
    /// Create a new SQLite storage with an existing pool
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// Create a new SQLite storage from a database URL
    pub async fn connect(database_url: &str) -> Result<Self, StorageError> {
        let pool = sqlx::SqlitePool::connect(database_url)
            .await
            .map_err(|e| StorageError::Backend(format!("Failed to connect: {}", e)))?;

        let storage = Self { pool };
        storage.init().await?;
        Ok(storage)
    }

    /// Initialize the database schema
    async fn init(&self) -> Result<(), StorageError> {
        // Core entities table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                tags TEXT,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                version INTEGER NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create entities table: {}", e)))?;

        // Create index on status for faster filtering
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_entities_status ON entities(status)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create index: {}", e)))?;

        // Vector chunks table for semantic search
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS vector_chunks (
                id TEXT PRIMARY KEY,
                source_id TEXT NOT NULL,
                text TEXT NOT NULL,
                embedding BLOB NOT NULL,  -- Serialized f32 array
                position INTEGER NOT NULL,
                total_chunks INTEGER NOT NULL,
                metadata TEXT,  -- JSON
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create vector_chunks table: {}", e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_vector_chunks_source ON vector_chunks(source_id)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create vector index: {}", e)))?;

        // Chat messages table for conversation history
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chat_messages (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                metadata TEXT  -- JSON
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create chat_messages table: {}", e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_chat_messages_conversation ON chat_messages(conversation_id, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create chat index: {}", e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_chat_messages_user ON chat_messages(user_id, created_at DESC)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create user chat index: {}", e)))?;

        // Memories table for agent memory
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                conversation_id TEXT,
                content TEXT NOT NULL,
                memory_type TEXT NOT NULL,
                embedding BLOB,  -- Serialized f32 array, optional
                created_at TEXT NOT NULL,
                expires_at TEXT,  -- NULL = never
                metadata TEXT  -- JSON
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create memories table: {}", e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id, memory_type)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create memory index: {}", e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_memories_expires ON memories(expires_at) WHERE expires_at IS NOT NULL",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("Failed to create memory expires index: {}", e)))?;

        Ok(())
    }

    /// Convert a database row to an Entity
    fn row_to_entity(row: &sqlx::sqlite::SqliteRow) -> Result<Entity, StorageError> {
        use chrono::DateTime;
        use crate::core::models::{Metadata, Status};

        let id_str: String = row.get("id");
        let id = Id::parse(&id_str)
            .map_err(|_| StorageError::Serialization(format!("Invalid ID: {}", id_str)))?;

        let name: String = row.get("name");
        let description: Option<String> = row.get("description");

        let tags_str: String = row.get("tags");
        let tags: Vec<String> = serde_json::from_str(&tags_str)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        let status_str: String = row.get("status");
        let status = status_str.parse::<Status>()
            .map_err(|e| StorageError::Serialization(format!("Invalid status: {} - {}", status_str, e)))?;

        let created_at_str: String = row.get("created_at");
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map_err(|e| StorageError::Serialization(e.to_string()))?
            .with_timezone(&chrono::Utc);

        let updated_at_str: String = row.get("updated_at");
        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .map_err(|e| StorageError::Serialization(e.to_string()))?
            .with_timezone(&chrono::Utc);

        let version: i64 = row.get("version");

        Ok(Entity {
            id,
            name,
            description,
            tags: Some(tags),
            status,
            metadata: Metadata {
                created_at,
                updated_at,
                version: version as u64,
                tags: None,
            },
        })
    }
}

#[async_trait]
impl Storage for SqliteStorage {
    async fn get(&self, id: Id) -> Result<Entity, StorageError> {
        let row = sqlx::query(
            "SELECT * FROM entities WHERE id = ?1"
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(e.to_string()))?;

        match row {
            Some(row) => Self::row_to_entity(&row),
            None => Err(StorageError::NotFound(id)),
        }
    }

    async fn list(&self) -> Result<Vec<Entity>, StorageError> {
        let rows = sqlx::query(
            "SELECT * FROM entities ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(e.to_string()))?;

        rows.iter()
            .map(Self::row_to_entity)
            .collect()
    }

    async fn create(&self, entity: &Entity) -> Result<(), StorageError> {
        let tags_json = serde_json::to_string(&entity.tags)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        sqlx::query(
            r#"
            INSERT INTO entities (id, name, description, tags, status, created_at, updated_at, version)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#
        )
        .bind(entity.id.to_string())
        .bind(&entity.name)
        .bind(&entity.description)
        .bind(tags_json)
        .bind(entity.status.to_string())
        .bind(entity.metadata.created_at.to_rfc3339())
        .bind(entity.metadata.updated_at.to_rfc3339())
        .bind(entity.metadata.version as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn update(&self, entity: &Entity) -> Result<(), StorageError> {
        let tags_json = serde_json::to_string(&entity.tags)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;

        let result = sqlx::query(
            r#"
            UPDATE entities
            SET name = ?1, description = ?2, tags = ?3, status = ?4,
                updated_at = ?5, version = ?6
            WHERE id = ?7
            "#
        )
        .bind(&entity.name)
        .bind(&entity.description)
        .bind(tags_json)
        .bind(entity.status.to_string())
        .bind(entity.metadata.updated_at.to_rfc3339())
        .bind(entity.metadata.version as i64)
        .bind(entity.id.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(entity.id));
        }

        Ok(())
    }

    async fn delete(&self, id: Id) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM entities WHERE id = ?1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        if result.rows_affected() == 0 {
            return Err(StorageError::NotFound(id));
        }

        Ok(())
    }

    async fn count(&self) -> Result<usize, StorageError> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM entities")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        let count: i64 = row.get("count");
        Ok(count as usize)
    }

    async fn health_check(&self) -> Result<(), StorageError> {
        // Try to execute a simple query
        let _: (i64,) = sqlx::query_as("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(e.to_string()))?;

        Ok(())
    }
}

// ============== UNIFIED STORAGE TRAIT IMPLEMENTATIONS ==============

// Helper functions for serialization
fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn deserialize_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn system_time_to_rfc3339(time: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339()
}

fn rfc3339_to_system_time(s: &str) -> Option<std::time::SystemTime> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc).into())
}

#[async_trait]
impl VectorStore for SqliteStorage {
    async fn store_chunk(&self, chunk: EmbeddedChunk) -> crate::Result<()> {
        let metadata_json = chunk.metadata.as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default())
            .unwrap_or_default();

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO vector_chunks
            (id, source_id, text, embedding, position, total_chunks, metadata, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )
        .bind(&chunk.id)
        .bind(&chunk.source_id)
        .bind(&chunk.text)
        .bind(serialize_embedding(&chunk.embedding))
        .bind(chunk.position as i64)
        .bind(chunk.total_chunks as i64)
        .bind(metadata_json)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to store vector chunk".to_string(),
            cause: Some(Box::new(e)),
        })?;

        Ok(())
    }

    async fn search_similar(
        &self,
        query_embedding: &[f32],
        limit: usize,
        threshold: f32,
    ) -> crate::Result<Vec<(EmbeddedChunk, f32)>> {
        // Load all chunks and compute similarity in Rust
        // For large datasets, this should use sqlite-vec extension or similar
        let rows = sqlx::query(
            "SELECT id, source_id, text, embedding, position, total_chunks, metadata FROM vector_chunks"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to search vectors".to_string(),
            cause: Some(Box::new(e)),
        })?;

        let mut results: Vec<(EmbeddedChunk, f32)> = rows
            .into_iter()
            .filter_map(|row| {
                let embedding_bytes: Vec<u8> = row.get("embedding");
                let chunk_embedding = deserialize_embedding(&embedding_bytes);
                let similarity = crate::memory::cosine_similarity(query_embedding, &chunk_embedding);

                if similarity >= threshold {
                    let metadata: Option<String> = row.get("metadata");
                    let metadata = metadata.and_then(|m| serde_json::from_str(&m).ok());

                    let chunk = EmbeddedChunk {
                        id: row.get("id"),
                        source_id: row.get("source_id"),
                        text: row.get("text"),
                        embedding: chunk_embedding,
                        position: row.get::<i64, _>("position") as usize,
                        total_chunks: row.get::<i64, _>("total_chunks") as usize,
                        metadata,
                    };
                    Some((chunk, similarity))
                } else {
                    None
                }
            })
            .collect();

        // Sort by similarity descending
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        results.truncate(limit);

        Ok(results)
    }

    async fn delete_by_source(&self, source_id: &str) -> crate::Result<usize> {
        let result = sqlx::query("DELETE FROM vector_chunks WHERE source_id = ?1")
            .bind(source_id)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to delete vector chunks".to_string(),
                cause: Some(Box::new(e)),
            })?;

        Ok(result.rows_affected() as usize)
    }

    async fn stats(&self) -> crate::Result<VectorStoreStats> {
        let row = sqlx::query(
            "SELECT COUNT(*) as total, COUNT(DISTINCT source_id) as sources FROM vector_chunks"
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to get vector stats".to_string(),
            cause: Some(Box::new(e)),
        })?;

        let total_vectors: i64 = row.get("total");
        let total_sources: i64 = row.get("sources");

        // Get dimension from first chunk
        let first_chunk = sqlx::query("SELECT embedding FROM vector_chunks LIMIT 1")
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to get vector dimension".to_string(),
                cause: Some(Box::new(e)),
            })?;

        let dimension = first_chunk
            .map(|row| {
                let bytes: Vec<u8> = row.get("embedding");
                bytes.len() / 4  // 4 bytes per f32
            })
            .unwrap_or(0);

        Ok(VectorStoreStats {
            total_vectors: total_vectors as usize,
            total_sources: total_sources as usize,
            dimension,
        })
    }

    async fn clear(&self) -> crate::Result<()> {
        sqlx::query("DELETE FROM vector_chunks")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to clear vectors".to_string(),
                cause: Some(Box::new(e)),
            })?;
        Ok(())
    }
}

#[async_trait]
impl ChatHistoryStore for SqliteStorage {
    async fn store_message(&self, message: ChatMessage) -> crate::Result<()> {
        let metadata_json = message.metadata.as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default())
            .unwrap_or_default();

        sqlx::query(
            r#"
            INSERT INTO chat_messages
            (id, conversation_id, user_id, role, content, created_at, metadata)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )
        .bind(&message.id)
        .bind(&message.conversation_id)
        .bind(&message.user_id)
        .bind(&message.role)
        .bind(&message.content)
        .bind(system_time_to_rfc3339(message.created_at))
        .bind(metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to store chat message".to_string(),
            cause: Some(Box::new(e)),
        })?;

        Ok(())
    }

    async fn get_conversation_history(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> crate::Result<Vec<ChatMessage>> {
        let rows = sqlx::query(
            r#"
            SELECT id, conversation_id, user_id, role, content, created_at, metadata
            FROM chat_messages
            WHERE conversation_id = ?1
            ORDER BY created_at DESC
            LIMIT ?2
            "#,
        )
        .bind(conversation_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to get conversation history".to_string(),
            cause: Some(Box::new(e)),
        })?;

        let messages: Vec<ChatMessage> = rows
            .into_iter()
            .filter_map(|row| {
                let created_at_str: String = row.get("created_at");
                let created_at = rfc3339_to_system_time(&created_at_str)?;

                let metadata: Option<String> = row.get("metadata");
                let metadata = metadata.and_then(|m| serde_json::from_str(&m).ok());

                Some(ChatMessage {
                    id: row.get("id"),
                    conversation_id: row.get("conversation_id"),
                    user_id: row.get("user_id"),
                    role: row.get("role"),
                    content: row.get("content"),
                    created_at,
                    metadata,
                })
            })
            .rev()  // Reverse to get chronological order
            .collect();

        Ok(messages)
    }

    async fn get_user_conversations(
        &self,
        user_id: &str,
        limit: usize,
    ) -> crate::Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT conversation_id
            FROM chat_messages
            WHERE user_id = ?1
            ORDER BY created_at DESC
            LIMIT ?2
            "#,
        )
        .bind(user_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to get user conversations".to_string(),
            cause: Some(Box::new(e)),
        })?;

        let conversations: Vec<String> = rows
            .into_iter()
            .map(|row| row.get("conversation_id"))
            .collect();

        Ok(conversations)
    }

    async fn delete_conversation(&self, conversation_id: &str) -> crate::Result<()> {
        sqlx::query("DELETE FROM chat_messages WHERE conversation_id = ?1")
            .bind(conversation_id)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to delete conversation".to_string(),
                cause: Some(Box::new(e)),
            })?;
        Ok(())
    }

    async fn get_last_conversation(&self, user_id: &str) -> crate::Result<Option<String>> {
        let row = sqlx::query(
            r#"
            SELECT conversation_id FROM chat_messages
            WHERE user_id = ?1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to get last conversation".to_string(),
            cause: Some(Box::new(e)),
        })?;

        Ok(row.map(|r| r.get("conversation_id")))
    }
}

#[async_trait]
impl MemoryStore for SqliteStorage {
    async fn store(&self, memory: Memory) -> crate::Result<MemoryId> {
        let metadata_json = memory.metadata.as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default())
            .unwrap_or_default();

        let embedding_bytes = memory.embedding.as_ref().map(|e| serialize_embedding(e));

        let expires_at = memory.expires_at.map(|t| system_time_to_rfc3339(t));

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO memories
            (id, user_id, conversation_id, content, memory_type, embedding, created_at, expires_at, metadata)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
        )
        .bind(memory.id.to_string())
        .bind(&memory.user_id)
        .bind(&memory.conversation_id)
        .bind(&memory.content)
        .bind(&memory.memory_type)
        .bind(embedding_bytes)
        .bind(system_time_to_rfc3339(memory.created_at))
        .bind(expires_at)
        .bind(metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to store memory".to_string(),
            cause: Some(Box::new(e)),
        })?;

        Ok(memory.id)
    }

    async fn get(&self, id: &MemoryId) -> crate::Result<Option<Memory>> {
        let row = sqlx::query(
            "SELECT * FROM memories WHERE id = ?1"
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to get memory".to_string(),
            cause: Some(Box::new(e)),
        })?;

        match row {
            Some(row) => {
                let created_at_str: String = row.get("created_at");
                let created_at = rfc3339_to_system_time(&created_at_str)
                    .ok_or_else(|| crate::error::MantaError::Internal("Invalid created_at".to_string()))?;

                let expires_at = row.get::<Option<String>, _>("expires_at")
                    .and_then(|s| rfc3339_to_system_time(&s));

                let embedding = row.get::<Option<Vec<u8>>, _>("embedding")
                    .map(|b| deserialize_embedding(&b));

                let metadata = row.get::<Option<String>, _>("metadata")
                    .and_then(|m| serde_json::from_str(&m).ok());

                Ok(Some(Memory {
                    id: MemoryId::new(row.get::<String, _>("id")),
                    user_id: row.get("user_id"),
                    conversation_id: row.get("conversation_id"),
                    content: row.get("content"),
                    memory_type: row.get("memory_type"),
                    embedding,
                    created_at,
                    expires_at,
                    metadata,
                }))
            }
            None => Ok(None),
        }
    }

    async fn update(&self, memory: Memory) -> crate::Result<()> {
        // Use store since it uses INSERT OR REPLACE
        self.store(memory).await?;
        Ok(())
    }

    async fn delete(&self, id: &MemoryId) -> crate::Result<bool> {
        let result = sqlx::query("DELETE FROM memories WHERE id = ?1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to delete memory".to_string(),
                cause: Some(Box::new(e)),
            })?;

        Ok(result.rows_affected() > 0)
    }

    async fn search(&self, query: MemoryQuery) -> crate::Result<Vec<Memory>> {
        let mut sql = "SELECT * FROM memories WHERE 1=1".to_string();
        let mut params: Vec<String> = Vec::new();

        if let Some(user_id) = &query.user_id {
            sql.push_str(&format!(" AND user_id = ?{}", params.len() + 1));
            params.push(user_id.clone());
        }

        if let Some(conversation_id) = &query.conversation_id {
            sql.push_str(&format!(" AND conversation_id = ?{}", params.len() + 1));
            params.push(conversation_id.clone());
        }

        if let Some(memory_type) = &query.memory_type {
            sql.push_str(&format!(" AND memory_type = ?{}", params.len() + 1));
            params.push(memory_type.clone());
        }

        if let Some(content_query) = &query.content_query {
            sql.push_str(&format!(" AND content LIKE ?{}", params.len() + 1));
            params.push(format!("%{}%", content_query));
        }

        if !query.include_expired {
            sql.push_str(" AND (expires_at IS NULL OR expires_at > datetime('now'))");
        }

        sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ?{}", params.len() + 1));
        params.push(query.limit.to_string());

        let mut query_builder = sqlx::query(&sql);
        for param in params {
            query_builder = query_builder.bind(param);
        }

        let rows = query_builder
            .fetch_all(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Failed to search memories".to_string(),
                cause: Some(Box::new(e)),
            })?;

        let memories: Vec<Memory> = rows
            .into_iter()
            .filter_map(|row| {
                let created_at_str: String = row.get("created_at");
                let created_at = rfc3339_to_system_time(&created_at_str)?;

                let expires_at = row.get::<Option<String>, _>("expires_at")
                    .and_then(|s| rfc3339_to_system_time(&s));

                let embedding = row.get::<Option<Vec<u8>>, _>("embedding")
                    .map(|b| deserialize_embedding(&b));

                let metadata = row.get::<Option<String>, _>("metadata")
                    .and_then(|m| serde_json::from_str(&m).ok());

                Some(Memory {
                    id: MemoryId::new(row.get::<String, _>("id")),
                    user_id: row.get("user_id"),
                    conversation_id: row.get("conversation_id"),
                    content: row.get("content"),
                    memory_type: row.get("memory_type"),
                    embedding,
                    created_at,
                    expires_at,
                    metadata,
                })
            })
            .collect();

        // If semantic search with embedding, sort by similarity
        if let Some(query_embedding) = query.embedding {
            let mut scored: Vec<(Memory, f32)> = memories
                .into_iter()
                .filter_map(|m| {
                    m.embedding.clone().map(|e| {
                        let similarity = crate::memory::cosine_similarity(&query_embedding, &e);
                        (m, similarity)
                    })
                })
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            return Ok(scored.into_iter().map(|(m, _)| m).collect());
        }

        Ok(memories)
    }

    async fn cleanup_expired(&self) -> crate::Result<usize> {
        let result = sqlx::query(
            "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < datetime('now')"
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to cleanup expired memories".to_string(),
            cause: Some(Box::new(e)),
        })?;

        Ok(result.rows_affected() as usize)
    }

    async fn stats(&self) -> crate::Result<MemoryStats> {
        let row = sqlx::query(
            r#"
            SELECT
                COUNT(*) as total,
                SUM(CASE WHEN expires_at IS NOT NULL AND expires_at < datetime('now') THEN 1 ELSE 0 END) as expired
            FROM memories
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::ExternalService {
            source: "Failed to get memory stats".to_string(),
            cause: Some(Box::new(e)),
        })?;

        let total_count: i64 = row.get("total");
        let expired_count: i64 = row.get("expired");

        Ok(MemoryStats {
            total_count: total_count as usize,
            count_by_type: std::collections::HashMap::new(),
            expired_count: expired_count as usize,
        })
    }

    async fn close(&self) -> crate::Result<()> {
        self.pool.close().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::Entity;

    #[tokio::test]
    async fn test_in_memory_storage() {
        let storage = InMemoryStorage::new();

        // Test empty storage
        assert_eq!(storage.count().await.unwrap(), 0);

        // Create entity
        let entity = Entity::new("Test Entity");
        storage.create(&entity).await.unwrap();
        assert_eq!(storage.count().await.unwrap(), 1);

        // Get entity
        let retrieved = storage.get(entity.id).await.unwrap();
        assert_eq!(retrieved.id, entity.id);
        assert_eq!(retrieved.name, entity.name);

        // Update entity
        let mut updated = entity.clone();
        updated.set_name("Updated Name");
        storage.update(&updated).await.unwrap();

        let retrieved = storage.get(entity.id).await.unwrap();
        assert_eq!(retrieved.name, "Updated Name");

        // Delete entity
        storage.delete(entity.id).await.unwrap();
        assert_eq!(storage.count().await.unwrap(), 0);
        assert!(storage.get(entity.id).await.is_err());
    }

    #[tokio::test]
    async fn test_storage_capacity() {
        let storage = InMemoryStorage::with_capacity(2);

        storage.create(&Entity::new("Entity 1")).await.unwrap();
        storage.create(&Entity::new("Entity 2")).await.unwrap();

        // Third entity should fail
        assert!(storage.create(&Entity::new("Entity 3")).await.is_err());
    }
}
