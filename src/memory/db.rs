//! Unified SQLite memory store for Manta
//!
//! Promotes `DatabaseStore` as the single canonical store, implementing both
//! `MemoryStore` and `ChatHistoryStore`.  Features:
//! - WAL mode for better concurrency
//! - FTS5 full-text search with Porter stemmer
//! - Access tracking (`access_count`, `last_accessed`)
//! - `importance_score` and `source` columns
//! - Batch query optimisation and connection pooling

use async_trait::async_trait;
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use tracing::{debug, info, instrument};

use super::{
    cosine_similarity, ChatHistoryStore, ChatMessage, Memory, MemoryId, MemoryQuery, MemoryStats,
    MemoryStore,
};

/// Unified database store with WAL, FTS5, and access tracking
#[derive(Debug, Clone)]
pub struct DatabaseStore {
    pool: Pool<Sqlite>,
    batch_size: usize,
}

impl DatabaseStore {
    /// Create a new optimised database store
    pub async fn new(database_url: &str) -> crate::Result<Self> {
        info!("Initializing unified database store");

        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .min_connections(2)
            .acquire_timeout(Duration::from_secs(30))
            .idle_timeout(Duration::from_secs(600))
            .max_lifetime(Duration::from_secs(3600))
            .connect(database_url)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to connect to database".to_string(),
                details: e.to_string(),
            })?;

        let store = Self { pool, batch_size: 100 };

        store.optimize().await?;
        store.init_schema().await?;
        store.migrate_schema().await?;

        info!("Unified database store initialized");
        Ok(store)
    }

    /// Create an in-memory store (for testing)
    pub async fn new_in_memory() -> crate::Result<Self> {
        Self::new("sqlite::memory:").await
    }

    /// Create a store from an existing pool (for sharing connections with other services)
    pub async fn new_with_pool(pool: Pool<Sqlite>) -> crate::Result<Self> {
        info!("Initializing unified database store from existing pool");

        let store = Self { pool, batch_size: 100 };

        store.optimize().await?;
        store.init_schema().await?;
        store.migrate_schema().await?;

        info!("Unified database store initialized from pool");
        Ok(store)
    }

    /// Apply SQLite performance pragmas
    async fn optimize(&self) -> crate::Result<()> {
        debug!("Applying database optimizations");

        let pragmas = [
            ("journal_mode = WAL", "Failed to enable WAL mode"),
            ("foreign_keys = ON", "Failed to enable foreign keys"),
            ("synchronous = NORMAL", "Failed to set synchronous mode"),
            ("cache_size = -32000", "Failed to set cache size"),
            ("temp_store = MEMORY", "Failed to set temp store"),
            ("mmap_size = 33554432", "Failed to set mmap size"),
        ];

        for (pragma, context) in &pragmas {
            sqlx::query(&format!("PRAGMA {}", pragma))
                .execute(&self.pool)
                .await
                .map_err(|e| crate::error::MantaError::Storage {
                    context: context.to_string(),
                    details: e.to_string(),
                })?;
        }

        debug!("Database optimizations applied");
        Ok(())
    }

    /// Create tables, indexes, FTS5 virtual table, and triggers
    async fn init_schema(&self) -> crate::Result<()> {
        debug!("Creating unified database schema");

        // --- memories table ---
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                conversation_id TEXT,
                content TEXT NOT NULL,
                memory_type TEXT NOT NULL DEFAULT 'general',
                embedding BLOB,
                created_at INTEGER NOT NULL,
                expires_at INTEGER,
                metadata TEXT,
                access_count INTEGER DEFAULT 0,
                last_accessed INTEGER,
                importance_score REAL NOT NULL DEFAULT 0.5,
                source TEXT NOT NULL DEFAULT 'agent'
            ) WITHOUT ROWID
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create memories table".to_string(),
            details: e.to_string(),
        })?;

        // --- chat_messages table ---
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

        // --- indexes ---
        let indexes: &[(&str, &str)] = &[
            ("idx_memories_user",         "CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id)"),
            ("idx_memories_conv",         "CREATE INDEX IF NOT EXISTS idx_memories_conv ON memories(conversation_id)"),
            ("idx_memories_type",         "CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type)"),
            ("idx_memories_expires",      "CREATE INDEX IF NOT EXISTS idx_memories_expires ON memories(expires_at) WHERE expires_at IS NOT NULL"),
            ("idx_memories_created",      "CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at)"),
            ("idx_memories_user_type",    "CREATE INDEX IF NOT EXISTS idx_memories_user_type ON memories(user_id, memory_type)"),
            ("idx_memories_last_accessed","CREATE INDEX IF NOT EXISTS idx_memories_last_accessed ON memories(last_accessed)"),
            ("idx_memories_importance",   "CREATE INDEX IF NOT EXISTS idx_memories_importance ON memories(importance_score)"),
            ("idx_chat_conv",             "CREATE INDEX IF NOT EXISTS idx_chat_conv ON chat_messages(conversation_id)"),
            ("idx_chat_user",             "CREATE INDEX IF NOT EXISTS idx_chat_user ON chat_messages(user_id)"),
            ("idx_chat_created",          "CREATE INDEX IF NOT EXISTS idx_chat_created ON chat_messages(created_at)"),
        ];

        for (name, sql) in indexes {
            sqlx::query(sql).execute(&self.pool).await.map_err(|e| {
                crate::error::MantaError::Storage {
                    context: format!("Failed to create index {}", name),
                    details: e.to_string(),
                }
            })?;
        }

        // --- FTS5 virtual table for full-text search ---
        sqlx::query(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content,
                user_id UNINDEXED,
                memory_id UNINDEXED,
                tokenize='porter'
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create FTS5 table".to_string(),
            details: e.to_string(),
        })?;

        // --- triggers to keep FTS5 in sync ---
        let triggers: &[(&str, &str)] = &[
            ("memories_fts_insert", r#"
                CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
                    INSERT INTO memories_fts(content, user_id, memory_id)
                    VALUES (NEW.content, NEW.user_id, NEW.id);
                END"#),
            ("memories_fts_delete", r#"
                CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
                    DELETE FROM memories_fts WHERE memory_id = OLD.id;
                END"#),
            ("memories_fts_update", r#"
                CREATE TRIGGER IF NOT EXISTS memories_fts_update AFTER UPDATE ON memories BEGIN
                    DELETE FROM memories_fts WHERE memory_id = OLD.id;
                    INSERT INTO memories_fts(content, user_id, memory_id)
                    VALUES (NEW.content, NEW.user_id, NEW.id);
                END"#),
        ];

        for (name, sql) in triggers {
            sqlx::query(sql).execute(&self.pool).await.map_err(|e| {
                crate::error::MantaError::Storage {
                    context: format!("Failed to create trigger {}", name),
                    details: e.to_string(),
                }
            })?;
        }

        debug!("Unified schema created");
        Ok(())
    }

    /// Add new columns to databases created before this migration (idempotent)
    async fn migrate_schema(&self) -> crate::Result<()> {
        // Errors are intentionally ignored — they fire when the column already exists
        let _ = sqlx::query(
            "ALTER TABLE memories ADD COLUMN importance_score REAL NOT NULL DEFAULT 0.5",
        )
        .execute(&self.pool)
        .await;

        let _ = sqlx::query(
            "ALTER TABLE memories ADD COLUMN source TEXT NOT NULL DEFAULT 'agent'",
        )
        .execute(&self.pool)
        .await;

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Public helpers (used by tests and the type alias in sqlite.rs)
    // -------------------------------------------------------------------------

    /// Serialise an f32 embedding slice to little-endian bytes
    pub fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Deserialise little-endian bytes back to an f32 embedding
    pub fn deserialize_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| {
                let arr: [u8; 4] = chunk.try_into().unwrap_or([0; 4]);
                f32::from_le_bytes(arr)
            })
            .collect()
    }

    /// Get the underlying connection pool
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    /// Set batch size for bulk operations
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    fn system_time_to_secs(time: SystemTime) -> i64 {
        time.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    fn secs_to_system_time(secs: i64) -> Option<SystemTime> {
        if secs <= 0 {
            None
        } else {
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs as u64))
        }
    }

    #[allow(clippy::too_many_arguments)]
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
        importance_score: f32,
        source: String,
    ) -> crate::Result<Memory> {
        let embedding = embedding_bytes.map(|b| Self::deserialize_embedding(&b));
        let created_at =
            Self::secs_to_system_time(created_at_secs).unwrap_or_else(SystemTime::now);
        let expires_at = expires_at_secs.and_then(Self::secs_to_system_time);
        let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());

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
            importance_score,
            source,
        })
    }

    // -------------------------------------------------------------------------
    // Maintenance operations
    // -------------------------------------------------------------------------

    /// Run ANALYZE to update statistics for query optimizer
    #[instrument(skip(self))]
    pub async fn analyze(&self) -> crate::Result<()> {
        info!("Running ANALYZE to update query statistics");
        sqlx::query("ANALYZE")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to run ANALYZE".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }

    /// Run VACUUM to optimise the database file
    #[instrument(skip(self))]
    pub async fn vacuum(&self) -> crate::Result<()> {
        info!("Running VACUUM to optimize database");
        sqlx::query("VACUUM")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to run VACUUM".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }

    /// Get low-level database statistics
    pub async fn db_stats(&self) -> crate::Result<DbStats> {
        let page_count: i64 = sqlx::query_scalar("PRAGMA page_count")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to get page count".to_string(),
                details: e.to_string(),
            })?;

        let freelist_count: i64 = sqlx::query_scalar("PRAGMA freelist_count")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to get freelist count".to_string(),
                details: e.to_string(),
            })?;

        let page_size: i64 = sqlx::query_scalar("PRAGMA page_size")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to get page size".to_string(),
                details: e.to_string(),
            })?;

        let user_version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to get user version".to_string(),
                details: e.to_string(),
            })?;

        Ok(DbStats {
            page_count,
            freelist_count,
            page_size,
            user_version,
            database_size_bytes: page_count * page_size,
        })
    }
}

// =============================================================================
// MemoryStore implementation
// =============================================================================

#[async_trait]
impl MemoryStore for DatabaseStore {
    async fn store(&self, memory: Memory) -> crate::Result<MemoryId> {
        debug!("Storing memory: {}", memory.id);

        let embedding_bytes =
            memory.embedding.as_ref().map(|e| Self::serialize_embedding(e));
        let created_at_secs = Self::system_time_to_secs(memory.created_at);
        let expires_at_secs = memory.expires_at.map(Self::system_time_to_secs);
        let metadata_str = memory
            .metadata
            .as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());

        sqlx::query(
            r#"
            INSERT INTO memories
            (id, user_id, conversation_id, content, memory_type, embedding,
             created_at, expires_at, metadata, importance_score, source)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
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
        .bind(memory.importance_score)
        .bind(&memory.source)
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
            "SELECT id, user_id, conversation_id, content, memory_type, embedding, \
             created_at, expires_at, metadata, importance_score, source \
             FROM memories WHERE id = ?",
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
                // Update access tracking (best-effort)
                let now = Self::system_time_to_secs(SystemTime::now());
                let _ = sqlx::query(
                    "UPDATE memories SET access_count = access_count + 1, last_accessed = ? WHERE id = ?",
                )
                .bind(now)
                .bind(&id.0)
                .execute(&self.pool)
                .await;

                let memory = Self::build_memory(
                    row.try_get("id").map_err(|e| col_err("id", e))?,
                    row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
                    row.try_get("conversation_id")
                        .map_err(|e| col_err("conversation_id", e))?,
                    row.try_get("content").map_err(|e| col_err("content", e))?,
                    row.try_get("memory_type")
                        .map_err(|e| col_err("memory_type", e))?,
                    row.try_get("embedding").map_err(|e| col_err("embedding", e))?,
                    row.try_get("created_at")
                        .map_err(|e| col_err("created_at", e))?,
                    row.try_get("expires_at")
                        .map_err(|e| col_err("expires_at", e))?,
                    row.try_get("metadata").map_err(|e| col_err("metadata", e))?,
                    row.try_get("importance_score")
                        .map_err(|e| col_err("importance_score", e))?,
                    row.try_get("source").map_err(|e| col_err("source", e))?,
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

        let embedding_bytes =
            memory.embedding.as_ref().map(|e| Self::serialize_embedding(e));
        let created_at_secs = Self::system_time_to_secs(memory.created_at);
        let expires_at_secs = memory.expires_at.map(Self::system_time_to_secs);
        let metadata_str = memory
            .metadata
            .as_ref()
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
                metadata = ?,
                importance_score = ?,
                source = ?
            WHERE id = ?
            "#,
        )
        .bind(&memory.user_id)
        .bind(&memory.conversation_id)
        .bind(&memory.content)
        .bind(&memory.memory_type)
        .bind(embedding_bytes)
        .bind(created_at_secs)
        .bind(expires_at_secs)
        .bind(metadata_str)
        .bind(memory.importance_score)
        .bind(&memory.source)
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
        debug!("Searching memories");

        let mut sql = "SELECT id, user_id, conversation_id, content, memory_type, embedding, \
                       created_at, expires_at, metadata, importance_score, source \
                       FROM memories WHERE 1=1"
            .to_string();

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

        let fetch_limit = if query.embedding.is_some() { query.limit * 10 } else { query.limit };
        sql.push_str(&format!(
            " ORDER BY importance_score DESC, created_at DESC LIMIT {} OFFSET {}",
            fetch_limit, query.offset
        ));

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

        let rows = db_query.fetch_all(&self.pool).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: "Failed to search memories".to_string(),
                details: e.to_string(),
            }
        })?;

        let mut memories: Vec<Memory> = Vec::with_capacity(rows.len());
        for row in rows {
            let memory = Self::build_memory(
                row.try_get("id").map_err(|e| col_err("id", e))?,
                row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
                row.try_get("conversation_id")
                    .map_err(|e| col_err("conversation_id", e))?,
                row.try_get("content").map_err(|e| col_err("content", e))?,
                row.try_get("memory_type")
                    .map_err(|e| col_err("memory_type", e))?,
                row.try_get("embedding").map_err(|e| col_err("embedding", e))?,
                row.try_get("created_at")
                    .map_err(|e| col_err("created_at", e))?,
                row.try_get("expires_at")
                    .map_err(|e| col_err("expires_at", e))?,
                row.try_get("metadata").map_err(|e| col_err("metadata", e))?,
                row.try_get("importance_score")
                    .map_err(|e| col_err("importance_score", e))?,
                row.try_get("source").map_err(|e| col_err("source", e))?,
            )?;
            memories.push(memory);
        }

        // Semantic re-ranking when an embedding query is provided
        if let Some(query_emb) = &query.embedding {
            let mut scored: Vec<(Memory, f32)> = memories
                .into_iter()
                .filter_map(|m| {
                    m.embedding.clone().map(|e| {
                        let score = cosine_similarity(query_emb, &e);
                        (m, score)
                    })
                })
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(query.limit);
            memories = scored.into_iter().map(|(m, _)| m).collect();
        }

        Ok(memories)
    }

    async fn cleanup_expired(&self) -> crate::Result<usize> {
        debug!("Cleaning up expired memories");

        let now = Self::system_time_to_secs(SystemTime::now());
        let result =
            sqlx::query("DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?")
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

        let total_row = sqlx::query("SELECT COUNT(*) as count FROM memories")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to get total count".to_string(),
                details: e.to_string(),
            })?;
        let total_count: i64 = total_row.try_get("count").map_err(|e| col_err("count", e))?;

        let type_rows =
            sqlx::query("SELECT memory_type, COUNT(*) as count FROM memories GROUP BY memory_type")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| crate::error::MantaError::Storage {
                    context: "Failed to get type counts".to_string(),
                    details: e.to_string(),
                })?;

        let mut count_by_type = HashMap::new();
        for row in type_rows {
            let mem_type: String =
                row.try_get("memory_type").map_err(|e| col_err("memory_type", e))?;
            let count: i64 = row.try_get("count").map_err(|e| col_err("count", e))?;
            count_by_type.insert(mem_type, count as usize);
        }

        let now = Self::system_time_to_secs(SystemTime::now());
        let expired_row = sqlx::query(
            "SELECT COUNT(*) as count FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?",
        )
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to get expired count".to_string(),
            details: e.to_string(),
        })?;
        let expired_count: i64 =
            expired_row.try_get("count").map_err(|e| col_err("count", e))?;

        Ok(MemoryStats {
            total_count: total_count as usize,
            count_by_type,
            expired_count: expired_count as usize,
        })
    }

    async fn close(&self) -> crate::Result<()> {
        debug!("Closing database connection pool");
        self.pool.close().await;
        Ok(())
    }
}

// =============================================================================
// ChatHistoryStore implementation
// =============================================================================

#[async_trait]
impl ChatHistoryStore for DatabaseStore {
    async fn store_message(&self, message: ChatMessage) -> crate::Result<()> {
        debug!(
            "Storing chat message {} in conversation {}",
            message.id, message.conversation_id
        );

        let created_at_secs = Self::system_time_to_secs(message.created_at);
        let metadata_str = message
            .metadata
            .as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());

        sqlx::query(
            r#"
            INSERT INTO chat_messages
            (id, conversation_id, user_id, role, content, created_at, metadata)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
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
            "#,
        )
        .bind(conversation_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to get conversation history".to_string(),
            details: e.to_string(),
        })?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            let created_at_secs: i64 =
                row.try_get("created_at").map_err(|e| col_err("created_at", e))?;
            let metadata_str: Option<String> =
                row.try_get("metadata").map_err(|e| col_err("metadata", e))?;

            messages.push(ChatMessage {
                id: row.try_get("id").map_err(|e| col_err("id", e))?,
                conversation_id: row
                    .try_get("conversation_id")
                    .map_err(|e| col_err("conversation_id", e))?,
                user_id: row.try_get("user_id").map_err(|e| col_err("user_id", e))?,
                role: row.try_get("role").map_err(|e| col_err("role", e))?,
                content: row.try_get("content").map_err(|e| col_err("content", e))?,
                created_at: Self::secs_to_system_time(created_at_secs)
                    .unwrap_or_else(SystemTime::now),
                metadata: metadata_str.and_then(|s| serde_json::from_str(&s).ok()),
            });
        }

        Ok(messages)
    }

    async fn get_user_conversations(
        &self,
        user_id: &str,
        limit: usize,
    ) -> crate::Result<Vec<String>> {
        debug!("Getting conversations for user: {}", user_id);

        // Use a subquery to get distinct conversations ordered by latest message
        let rows = sqlx::query(
            r#"
            SELECT conversation_id
            FROM (
                SELECT conversation_id, MAX(created_at) as last_msg
                FROM chat_messages
                WHERE user_id = ?
                GROUP BY conversation_id
            )
            ORDER BY last_msg DESC
            LIMIT ?
            "#,
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
            "#,
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

// =============================================================================
// Supporting types
// =============================================================================

/// Low-level database page statistics
#[derive(Debug, Clone)]
pub struct DbStats {
    pub page_count: i64,
    pub freelist_count: i64,
    pub page_size: i64,
    pub user_version: i64,
    pub database_size_bytes: i64,
}

impl DbStats {
    pub fn fragmentation_percent(&self) -> f64 {
        if self.page_count == 0 {
            return 0.0;
        }
        (self.freelist_count as f64 / self.page_count as f64) * 100.0
    }

    pub fn size_formatted(&self) -> String {
        let bytes = self.database_size_bytes as f64;
        if bytes < 1024.0 {
            format!("{:.0} B", bytes)
        } else if bytes < 1024.0 * 1024.0 {
            format!("{:.2} KB", bytes / 1024.0)
        } else if bytes < 1024.0 * 1024.0 * 1024.0 {
            format!("{:.2} MB", bytes / (1024.0 * 1024.0))
        } else {
            format!("{:.2} GB", bytes / (1024.0 * 1024.0 * 1024.0))
        }
    }
}

/// Query builder for complex SQL queries
#[derive(Debug)]
pub struct QueryBuilder {
    base: String,
    conditions: Vec<String>,
    order_by: Vec<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

impl QueryBuilder {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            conditions: Vec::new(),
            order_by: Vec::new(),
            limit: None,
            offset: None,
        }
    }

    pub fn and_where(mut self, condition: impl Into<String>) -> Self {
        self.conditions.push(condition.into());
        self
    }

    pub fn order_by(mut self, column: impl Into<String>, ascending: bool) -> Self {
        let dir = if ascending { "ASC" } else { "DESC" };
        self.order_by.push(format!("{} {}", column.into(), dir));
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    pub fn build(self) -> String {
        let mut query = self.base;

        if !self.conditions.is_empty() {
            query.push_str(" WHERE ");
            query.push_str(&self.conditions.join(" AND "));
        }

        if !self.order_by.is_empty() {
            query.push_str(" ORDER BY ");
            query.push_str(&self.order_by.join(", "));
        }

        if let Some(limit) = self.limit {
            query.push_str(&format!(" LIMIT {}", limit));
        }

        if let Some(offset) = self.offset {
            query.push_str(&format!(" OFFSET {}", offset));
        }

        query
    }
}

// =============================================================================
// Internal helpers
// =============================================================================

fn col_err(column: &str, err: sqlx::Error) -> crate::error::MantaError {
    crate::error::MantaError::Storage {
        context: format!("Failed to read column '{}'", column),
        details: err.to_string(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_builder() {
        let query = QueryBuilder::new("SELECT * FROM memories")
            .and_where("user_id = 'user1'")
            .and_where("created_at > 1000")
            .order_by("created_at", false)
            .limit(10)
            .offset(0)
            .build();

        assert!(query.contains("WHERE"));
        assert!(query.contains("user_id = 'user1'"));
        assert!(query.contains("ORDER BY"));
        assert!(query.contains("LIMIT 10"));
    }

    #[test]
    fn test_db_stats() {
        let stats = DbStats {
            page_count: 1000,
            freelist_count: 50,
            page_size: 4096,
            user_version: 1,
            database_size_bytes: 4096000,
        };

        assert_eq!(stats.fragmentation_percent(), 5.0);
        assert_eq!(stats.size_formatted(), "3.91 MB");
    }

    #[test]
    fn test_serialize_embedding() {
        let embedding = vec![1.0f32, 2.0, 3.0, 4.0];
        let bytes = DatabaseStore::serialize_embedding(&embedding);
        let deserialized = DatabaseStore::deserialize_embedding(&bytes);
        assert_eq!(embedding, deserialized);
    }

    #[tokio::test]
    async fn test_database_store_memory_crud() {
        let store = DatabaseStore::new_in_memory().await.unwrap();

        let memory = Memory::new("user1", "Hello world", "fact").with_conversation("conv1");
        let id = store.store(memory.clone()).await.unwrap();
        assert_eq!(id.0, memory.id.0);

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Hello world");
        assert_eq!(retrieved.user_id, "user1");
        assert_eq!(retrieved.importance_score, 0.5);
        assert_eq!(retrieved.source, "agent");

        let mut updated = retrieved.clone();
        updated.content = "Updated content".to_string();
        updated.importance_score = 0.9;
        store.update(updated).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Updated content");
        assert_eq!(retrieved.importance_score, 0.9);

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);

        let retrieved = store.get(&id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_database_store_search() {
        let store = DatabaseStore::new_in_memory().await.unwrap();

        for i in 0..5 {
            let memory = Memory::new("user1", &format!("Memory {}", i), "fact");
            store.store(memory).await.unwrap();
        }

        let results = store
            .search(MemoryQuery::new().for_user("user1").limit(10))
            .await
            .unwrap();
        assert_eq!(results.len(), 5);

        let results = store
            .search(MemoryQuery::new().with_content("Memory 2"))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Memory 2");
    }

    #[tokio::test]
    async fn test_database_store_expiration() {
        let store = DatabaseStore::new_in_memory().await.unwrap();

        let memory = Memory::new("user1", "Temporary", "fact").with_ttl(1);
        let id = store.store(memory).await.unwrap();

        assert!(store.get(&id).await.unwrap().is_some());

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        assert!(store.get(&id).await.unwrap().is_none());

        let cleaned = store.cleanup_expired().await.unwrap();
        assert_eq!(cleaned, 1);
    }

    #[tokio::test]
    async fn test_database_store_chat_history() {
        let store = DatabaseStore::new_in_memory().await.unwrap();

        let msg = ChatMessage::new("conv1", "user1", "user", "Hello!");
        store.store_message(msg).await.unwrap();

        let msg = ChatMessage::new("conv1", "user1", "assistant", "Hi there!");
        store.store_message(msg).await.unwrap();

        let history = store.get_conversation_history("conv1", 10).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].role, "assistant");

        let convs = store.get_user_conversations("user1", 10).await.unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0], "conv1");

        let last = store.get_last_conversation("user1").await.unwrap();
        assert_eq!(last, Some("conv1".to_string()));

        store.delete_conversation("conv1").await.unwrap();
        let history = store.get_conversation_history("conv1", 10).await.unwrap();
        assert!(history.is_empty());
    }
}
