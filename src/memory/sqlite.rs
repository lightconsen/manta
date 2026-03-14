//! SQLite implementation of the MemoryStore trait

use super::{ChatHistoryStore, ChatMessage, Memory, MemoryId, MemoryQuery, MemoryStats, MemoryStore, cosine_similarity};
use async_trait::async_trait;
use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite, Row};
use std::collections::HashMap;
use std::time::SystemTime;
use tracing::{debug, info};

/// SQLite-based memory store
#[derive(Debug, Clone)]
pub struct SqliteMemoryStore {
    pool: Pool<Sqlite>,
}

impl SqliteMemoryStore {
    /// Create a new SQLite memory store
    pub async fn new(database_url: &str) -> crate::Result<Self> {
        info!("Initializing SQLite memory store");

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to connect to SQLite database".to_string(),
                details: e.to_string(),
            })?;

        let store = Self { pool };
        store.init().await?;

        info!("SQLite memory store initialized");
        Ok(store)
    }

    /// Create a new in-memory SQLite store (for testing)
    pub async fn new_in_memory() -> crate::Result<Self> {
        Self::new("sqlite::memory:").await
    }

    /// Get the database pool for use with other components (like SessionSearch)
    pub fn pool(&self) -> Pool<Sqlite> {
        self.pool.clone()
    }

    /// Initialize the database schema
    async fn init(&self) -> crate::Result<()> {
        debug!("Creating database schema");

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                conversation_id TEXT,
                content TEXT NOT NULL,
                memory_type TEXT NOT NULL,
                embedding BLOB,
                created_at INTEGER NOT NULL,
                expires_at INTEGER,
                metadata TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create memories table".to_string(),
            details: e.to_string(),
        })?;

        // Create indexes
        for (idx_name, col) in [
            ("idx_memories_user", "user_id"),
            ("idx_memories_conv", "conversation_id"),
            ("idx_memories_type", "memory_type"),
            ("idx_memories_expires", "expires_at"),
        ] {
            let sql = format!(
                "CREATE INDEX IF NOT EXISTS {} ON memories({})",
                idx_name, col
            );
            sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .map_err(|e| crate::error::MantaError::Storage {
                    context: format!("Failed to create index {}", idx_name),
                    details: e.to_string(),
                })?;
        }

        // Create chat_messages table for conversation history
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chat_messages (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                metadata TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create chat_messages table".to_string(),
            details: e.to_string(),
        })?;

        // Create indexes for chat_messages
        for (idx_name, col) in [
            ("idx_chat_conv", "conversation_id"),
            ("idx_chat_user", "user_id"),
            ("idx_chat_created", "created_at"),
        ] {
            let sql = format!(
                "CREATE INDEX IF NOT EXISTS {} ON chat_messages({})",
                idx_name, col
            );
            sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .map_err(|e| crate::error::MantaError::Storage {
                    context: format!("Failed to create index {}", idx_name),
                    details: e.to_string(),
                })?;
        }

        debug!("Database schema created successfully");
        Ok(())
    }

    /// Serialize embedding to bytes
    fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
        embedding.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect()
    }

    /// Deserialize embedding from bytes
    fn deserialize_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes.chunks_exact(4)
            .map(|chunk| {
                let arr: [u8; 4] = chunk.try_into().unwrap_or([0; 4]);
                f32::from_le_bytes(arr)
            })
            .collect()
    }

    /// Convert SystemTime to Unix timestamp (seconds)
    fn system_time_to_secs(time: SystemTime) -> i64 {
        time.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    /// Convert Unix timestamp to SystemTime
    fn secs_to_system_time(secs: i64) -> Option<SystemTime> {
        if secs <= 0 {
            None
        } else {
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64))
        }
    }

    /// Build memory from row fields
    fn build_memory(
        id: String,
        user_id: String,
        conversation_id: Option<String>,
        content: String,
        memory_type: String,
        embedding_bytes: Option<Vec<u8>>,
        created_at_secs: i64,
        expires_at_secs: Option<i64>,
        metadata_str: Option<String>,
    ) -> crate::Result<Memory> {
        let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));
        let created_at = Self::secs_to_system_time(created_at_secs)
            .unwrap_or_else(SystemTime::now);
        let expires_at = expires_at_secs.and_then(Self::secs_to_system_time);
        let metadata = metadata_str
            .and_then(|s| serde_json::from_str(&s).ok());

        Ok(Memory {
            id: MemoryId::new(id),
            user_id,
            conversation_id,
            content,
            memory_type,
            embedding,
            created_at,
            expires_at,
            metadata,
        })
    }
}

#[async_trait]
impl MemoryStore for SqliteMemoryStore {
    async fn store(&self, memory: Memory) -> crate::Result<MemoryId> {
        debug!("Storing memory: {}", memory.id);

        let embedding_bytes = memory.embedding.as_ref()
            .map(|e| Self::serialize_embedding(e));

        let created_at_secs = Self::system_time_to_secs(memory.created_at);
        let expires_at_secs = memory.expires_at.map(Self::system_time_to_secs);
        let metadata_str = memory.metadata.as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());

        sqlx::query(
            r#"
            INSERT INTO memories
            (id, user_id, conversation_id, content, memory_type, embedding, created_at, expires_at, metadata)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(&memory.id.0)
        .bind(&memory.user_id)
        .bind(&memory.conversation_id)
        .bind(&memory.content)
        .bind(&memory.memory_type)
        .bind(embedding_bytes)
        .bind(created_at_secs)
        .bind(expires_at_secs)
        .bind(metadata_str)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to store memory".to_string(),
            details: e.to_string(),
        })?;

        info!("Memory stored: {}", memory.id);
        Ok(memory.id)
    }

    async fn get(&self, id: &MemoryId) -> crate::Result<Option<Memory>> {
        debug!("Getting memory: {}", id);

        let row = sqlx::query(
            "SELECT id, user_id, conversation_id, content, memory_type, embedding, created_at, expires_at, metadata FROM memories WHERE id = ?"
        )
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to get memory".to_string(),
                details: e.to_string(),
            })?;

        match row {
            Some(row) => {
                let memory = Self::build_memory(
                    row.try_get("id").map_err(|e| storage_err("id", e))?,
                    row.try_get("user_id").map_err(|e| storage_err("user_id", e))?,
                    row.try_get("conversation_id").map_err(|e| storage_err("conversation_id", e))?,
                    row.try_get("content").map_err(|e| storage_err("content", e))?,
                    row.try_get("memory_type").map_err(|e| storage_err("memory_type", e))?,
                    row.try_get("embedding").map_err(|e| storage_err("embedding", e))?,
                    row.try_get("created_at").map_err(|e| storage_err("created_at", e))?,
                    row.try_get("expires_at").map_err(|e| storage_err("expires_at", e))?,
                    row.try_get("metadata").map_err(|e| storage_err("metadata", e))?,
                )?;

                if memory.is_expired() {
                    Ok(None)
                } else {
                    Ok(Some(memory))
                }
            }
            None => Ok(None),
        }
    }

    async fn update(&self, memory: Memory) -> crate::Result<()> {
        debug!("Updating memory: {}", memory.id);

        let embedding_bytes = memory.embedding.as_ref()
            .map(|e| Self::serialize_embedding(e));
        let created_at_secs = Self::system_time_to_secs(memory.created_at);
        let expires_at_secs = memory.expires_at.map(Self::system_time_to_secs);
        let metadata_str = memory.metadata.as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());

        let result = sqlx::query(
            r#"
            UPDATE memories SET
                user_id = ?,
                conversation_id = ?,
                content = ?,
                memory_type = ?,
                embedding = ?,
                created_at = ?,
                expires_at = ?,
                metadata = ?
            WHERE id = ?
            "#
        )
        .bind(&memory.user_id)
        .bind(&memory.conversation_id)
        .bind(&memory.content)
        .bind(&memory.memory_type)
        .bind(embedding_bytes)
        .bind(created_at_secs)
        .bind(expires_at_secs)
        .bind(metadata_str)
        .bind(&memory.id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to update memory".to_string(),
            details: e.to_string(),
        })?;

        if result.rows_affected() == 0 {
            return Err(crate::error::MantaError::NotFound {
                resource: format!("Memory with id {}", memory.id),
            });
        }

        info!("Memory updated: {}", memory.id);
        Ok(())
    }

    async fn delete(&self, id: &MemoryId) -> crate::Result<bool> {
        debug!("Deleting memory: {}", id);

        let result = sqlx::query("DELETE FROM memories WHERE id = ?")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to delete memory".to_string(),
                details: e.to_string(),
            })?;

        let deleted = result.rows_affected() > 0;
        if deleted {
            info!("Memory deleted: {}", id);
        }
        Ok(deleted)
    }

    async fn search(&self, query: MemoryQuery) -> crate::Result<Vec<Memory>> {
        debug!("Searching memories with query");

        // Build query dynamically based on filters
        let mut sql = "SELECT id, user_id, conversation_id, content, memory_type, embedding, created_at, expires_at, metadata FROM memories WHERE 1=1".to_string();

        if query.user_id.is_some() {
            sql.push_str(" AND user_id = ?");
        }
        if query.conversation_id.is_some() {
            sql.push_str(" AND conversation_id = ?");
        }
        if query.memory_type.is_some() {
            sql.push_str(" AND memory_type = ?");
        }
        if query.content_query.is_some() {
            sql.push_str(" AND content LIKE ?");
        }
        if !query.include_expired {
            sql.push_str(" AND (expires_at IS NULL OR expires_at > ?)");
        }

        // If using semantic search, fetch more results to filter
        let fetch_limit = if query.embedding.is_some() {
            query.limit * 10
        } else {
            query.limit
        };

        sql.push_str(&format!(" ORDER BY created_at DESC LIMIT {} OFFSET {}", fetch_limit, query.offset));

        // Build and execute query
        let mut db_query = sqlx::query(&sql);

        if let Some(user_id) = &query.user_id {
            db_query = db_query.bind(user_id);
        }
        if let Some(conv_id) = &query.conversation_id {
            db_query = db_query.bind(conv_id);
        }
        if let Some(mem_type) = &query.memory_type {
            db_query = db_query.bind(mem_type);
        }
        if let Some(content) = &query.content_query {
            db_query = db_query.bind(format!("%{}%", content));
        }
        if !query.include_expired {
            let now = Self::system_time_to_secs(SystemTime::now());
            db_query = db_query.bind(now);
        }

        let rows = db_query.fetch_all(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to search memories".to_string(),
                details: e.to_string(),
            })?;

        let mut memories: Vec<Memory> = Vec::new();

        for row in rows {
            let memory = Self::build_memory(
                row.try_get("id").map_err(|e| storage_err("id", e))?,
                row.try_get("user_id").map_err(|e| storage_err("user_id", e))?,
                row.try_get("conversation_id").map_err(|e| storage_err("conversation_id", e))?,
                row.try_get("content").map_err(|e| storage_err("content", e))?,
                row.try_get("memory_type").map_err(|e| storage_err("memory_type", e))?,
                row.try_get("embedding").map_err(|e| storage_err("embedding", e))?,
                row.try_get("created_at").map_err(|e| storage_err("created_at", e))?,
                row.try_get("expires_at").map_err(|e| storage_err("expires_at", e))?,
                row.try_get("metadata").map_err(|e| storage_err("metadata", e))?,
            )?;
            memories.push(memory);
        }

        // If embedding search, score and re-rank
        if let Some(query_emb) = &query.embedding {
            let mut scored: Vec<(Memory, f32)> = memories
                .into_iter()
                .filter_map(|m| {
                    // Clone the embedding to avoid borrow issues
                    m.embedding.clone().map(|e| {
                        let score = cosine_similarity(query_emb, &e);
                        (m, score)
                    })
                })
                .collect();

            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            scored.truncate(query.limit);
            memories = scored.into_iter().map(|(m, _)| m).collect();
        }

        Ok(memories)
    }

    async fn cleanup_expired(&self) -> crate::Result<usize> {
        debug!("Cleaning up expired memories");

        let now = Self::system_time_to_secs(SystemTime::now());

        let result = sqlx::query("DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?")
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to cleanup expired memories".to_string(),
                details: e.to_string(),
            })?;

        let count = result.rows_affected() as usize;
        info!("Cleaned up {} expired memories", count);
        Ok(count)
    }

    async fn stats(&self) -> crate::Result<MemoryStats> {
        debug!("Getting memory stats");

        // Total count
        let total_row = sqlx::query("SELECT COUNT(*) as count FROM memories")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to get total count".to_string(),
                details: e.to_string(),
            })?;
        let total_count: i64 = total_row.try_get("count")
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to parse total count".to_string(),
                details: e.to_string(),
            })?;

        // Count by type
        let type_rows = sqlx::query("SELECT memory_type, COUNT(*) as count FROM memories GROUP BY memory_type")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to get type counts".to_string(),
                details: e.to_string(),
            })?;

        let mut count_by_type = HashMap::new();
        for row in type_rows {
            let mem_type: String = row.try_get("memory_type")
                .map_err(|e| crate::error::MantaError::Storage {
                    context: "Failed to parse memory_type".to_string(),
                    details: e.to_string(),
                })?;
            let count: i64 = row.try_get("count")
                .map_err(|e| crate::error::MantaError::Storage {
                    context: "Failed to parse count".to_string(),
                    details: e.to_string(),
                })?;
            count_by_type.insert(mem_type, count as usize);
        }

        // Expired count
        let now = Self::system_time_to_secs(SystemTime::now());
        let expired_row = sqlx::query("SELECT COUNT(*) as count FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?")
            .bind(now)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to get expired count".to_string(),
                details: e.to_string(),
            })?;
        let expired_count: i64 = expired_row.try_get("count")
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to parse expired count".to_string(),
                details: e.to_string(),
            })?;

        Ok(MemoryStats {
            total_count: total_count as usize,
            count_by_type,
            expired_count: expired_count as usize,
        })
    }

    async fn close(&self) -> crate::Result<()> {
        debug!("Closing SQLite connection pool");
        self.pool.close().await;
        Ok(())
    }
}

#[async_trait]
impl ChatHistoryStore for SqliteMemoryStore {
    async fn store_message(&self, message: ChatMessage) -> crate::Result<()> {
        debug!("Storing chat message: {} in conversation: {}", message.id, message.conversation_id);

        let created_at_secs = Self::system_time_to_secs(message.created_at);
        let metadata_str = message.metadata.as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());

        sqlx::query(
            r#"
            INSERT INTO chat_messages
            (id, conversation_id, user_id, role, content, created_at, metadata)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(&message.id)
        .bind(&message.conversation_id)
        .bind(&message.user_id)
        .bind(&message.role)
        .bind(&message.content)
        .bind(created_at_secs)
        .bind(metadata_str)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to store chat message".to_string(),
            details: e.to_string(),
        })?;

        info!("Chat message stored: {} in conversation: {}", message.id, message.conversation_id);
        Ok(())
    }

    async fn get_conversation_history(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> crate::Result<Vec<ChatMessage>> {
        debug!("Getting conversation history for: {}", conversation_id);

        let rows = sqlx::query(
            r#"
            SELECT id, conversation_id, user_id, role, content, created_at, metadata
            FROM chat_messages
            WHERE conversation_id = ?
            ORDER BY created_at ASC
            LIMIT ?
            "#
        )
        .bind(conversation_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to get conversation history".to_string(),
            details: e.to_string(),
        })?;

        let mut messages: Vec<ChatMessage> = Vec::new();
        for row in rows {
            let created_at_secs: i64 = row.try_get("created_at")
                .map_err(|e| storage_err("created_at", e))?;
            let metadata_str: Option<String> = row.try_get("metadata")
                .map_err(|e| storage_err("metadata", e))?;

            let message = ChatMessage {
                id: row.try_get("id").map_err(|e| storage_err("id", e))?,
                conversation_id: row.try_get("conversation_id").map_err(|e| storage_err("conversation_id", e))?,
                user_id: row.try_get("user_id").map_err(|e| storage_err("user_id", e))?,
                role: row.try_get("role").map_err(|e| storage_err("role", e))?,
                content: row.try_get("content").map_err(|e| storage_err("content", e))?,
                created_at: Self::secs_to_system_time(created_at_secs)
                    .unwrap_or_else(SystemTime::now),
                metadata: metadata_str.and_then(|s| serde_json::from_str(&s).ok()),
            };
            messages.push(message);
        }

        debug!("Retrieved {} messages for conversation: {}", messages.len(), conversation_id);
        Ok(messages)
    }

    async fn get_user_conversations(
        &self,
        user_id: &str,
        limit: usize,
    ) -> crate::Result<Vec<String>> {
        debug!("Getting conversations for user: {}", user_id);

        let rows = sqlx::query(
            r#"
            SELECT DISTINCT conversation_id
            FROM chat_messages
            WHERE user_id = ?
            ORDER BY MAX(created_at) DESC
            LIMIT ?
            "#
        )
        .bind(user_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to get user conversations".to_string(),
            details: e.to_string(),
        })?;

        let conversations: Vec<String> = rows
            .iter()
            .map(|row| row.try_get::<String, _>("conversation_id"))
            .filter_map(Result::ok)
            .collect();

        debug!("Retrieved {} conversations for user: {}", conversations.len(), user_id);
        Ok(conversations)
    }

    async fn delete_conversation(&self, conversation_id: &str) -> crate::Result<()> {
        debug!("Deleting conversation: {}", conversation_id);

        sqlx::query("DELETE FROM chat_messages WHERE conversation_id = ?")
            .bind(conversation_id)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to delete conversation".to_string(),
                details: e.to_string(),
            })?;

        info!("Conversation deleted: {}", conversation_id);
        Ok(())
    }

    async fn get_last_conversation(&self, user_id: &str) -> crate::Result<Option<String>> {
        debug!("Getting last conversation for user: {}", user_id);

        let row: Option<(String,)> = sqlx::query_as(
            r#"
            SELECT conversation_id FROM chat_messages
            WHERE user_id = ?
            ORDER BY created_at DESC
            LIMIT 1
            "#
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to get last conversation".to_string(),
            details: e.to_string(),
        })?;

        Ok(row.map(|r| r.0))
    }
}

/// Helper to create storage error
fn storage_err(column: &str, err: sqlx::Error) -> crate::error::MantaError {
    crate::error::MantaError::Storage {
        context: format!("Failed to get {} column", column),
        details: err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sqlite_memory_store() {
        let store = SqliteMemoryStore::new_in_memory().await.unwrap();

        // Store a memory
        let memory = Memory::new("user1", "Hello world", "fact")
            .with_conversation("conv1");

        let id = store.store(memory.clone()).await.unwrap();
        assert_eq!(id.0, memory.id.0);

        // Retrieve it
        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Hello world");
        assert_eq!(retrieved.user_id, "user1");

        // Update it
        let mut updated = retrieved.clone();
        updated.content = "Updated content".to_string();
        store.update(updated).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Updated content");

        // Delete it
        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);

        let retrieved = store.get(&id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_sqlite_search() {
        let store = SqliteMemoryStore::new_in_memory().await.unwrap();

        // Store multiple memories
        for i in 0..5 {
            let memory = Memory::new("user1", &format!("Memory {}", i), "fact");
            store.store(memory).await.unwrap();
        }

        // Search for user
        let results = store.search(MemoryQuery::new().for_user("user1").limit(10)).await.unwrap();
        assert_eq!(results.len(), 5);

        // Search with content filter
        let results = store.search(MemoryQuery::new().with_content("Memory 2")).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Memory 2");
    }

    #[tokio::test]
    async fn test_sqlite_expiration() {
        let store = SqliteMemoryStore::new_in_memory().await.unwrap();

        // Store a memory with very short TTL (1 second)
        let memory = Memory::new("user1", "Temporary", "fact")
            .with_ttl(1);

        let id = store.store(memory).await.unwrap();

        // Should be retrievable immediately
        let retrieved = store.get(&id).await.unwrap();
        assert!(retrieved.is_some());

        // Wait for expiration
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Should not be retrievable (expired)
        let retrieved = store.get(&id).await.unwrap();
        assert!(retrieved.is_none());

        // Cleanup should remove it
        let cleaned = store.cleanup_expired().await.unwrap();
        assert_eq!(cleaned, 1);
    }

    #[tokio::test]
    async fn test_serialize_embedding() {
        let embedding = vec![1.0f32, 2.0, 3.0, 4.0];
        let bytes = SqliteMemoryStore::serialize_embedding(&embedding);
        let deserialized = SqliteMemoryStore::deserialize_embedding(&bytes);
        assert_eq!(embedding, deserialized);
    }
}
