//! Database optimization utilities for SQLite
//!
//! Provides query optimization, batching, and indexing for improved performance.

use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite, Row};
use std::time::Duration;
use tracing::{debug, info, instrument};

/// Optimized database store with batching and query optimization
#[derive(Debug, Clone)]
pub struct DatabaseStore {
    pool: Pool<Sqlite>,
    batch_size: usize,
}

impl DatabaseStore {
    /// Create a new optimized database store
    pub async fn new(database_url: &str) -> crate::Result<Self> {
        info!("Initializing optimized database store");

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

        let store = Self {
            pool,
            batch_size: 100,
        };

        store.optimize().await?;
        store.init_schema().await?;

        info!("Optimized database store initialized");
        Ok(store)
    }

    /// Apply SQLite optimizations
    async fn optimize(&self) -> crate::Result<()> {
        debug!("Applying database optimizations");

        // Enable WAL mode for better concurrency
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to enable WAL mode".to_string(),
                details: e.to_string(),
            })?;

        // Enable foreign keys
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to enable foreign keys".to_string(),
                details: e.to_string(),
            })?;

        // Set synchronous mode to NORMAL for better performance
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to set synchronous mode".to_string(),
                details: e.to_string(),
            })?;

        // Increase cache size (negative value = KB)
        sqlx::query("PRAGMA cache_size = -32000")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to set cache size".to_string(),
                details: e.to_string(),
            })?;

        // Set temp store to memory
        sqlx::query("PRAGMA temp_store = MEMORY")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to set temp store".to_string(),
                details: e.to_string(),
            })?;

        // Enable memory-mapped I/O (32MB)
        sqlx::query("PRAGMA mmap_size = 33554432")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to set mmap size".to_string(),
                details: e.to_string(),
            })?;

        debug!("Database optimizations applied");
        Ok(())
    }

    /// Initialize schema with optimized indexes
    async fn init_schema(&self) -> crate::Result<()> {
        debug!("Creating optimized database schema");

        // Create memories table with optimized columns
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
                last_accessed INTEGER
            ) WITHOUT ROWID
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create memories table".to_string(),
            details: e.to_string(),
        })?;

        // Create optimized indexes
        let indexes = [
            ("idx_memories_user", "CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id)"),
            ("idx_memories_conv", "CREATE INDEX IF NOT EXISTS idx_memories_conv ON memories(conversation_id)"),
            ("idx_memories_type", "CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(memory_type)"),
            ("idx_memories_expires", "CREATE INDEX IF NOT EXISTS idx_memories_expires ON memories(expires_at) WHERE expires_at IS NOT NULL"),
            ("idx_memories_created", "CREATE INDEX IF NOT EXISTS idx_memories_created ON memories(created_at)"),
            ("idx_memories_user_type", "CREATE INDEX IF NOT EXISTS idx_memories_user_type ON memories(user_id, memory_type)"),
            ("idx_memories_last_accessed", "CREATE INDEX IF NOT EXISTS idx_memories_last_accessed ON memories(last_accessed)"),
        ];

        for (name, sql) in &indexes {
            sqlx::query(sql)
                .execute(&self.pool)
                .await
                .map_err(|e| crate::error::MantaError::Storage {
                    context: format!("Failed to create index {}", name),
                    details: e.to_string(),
                })?;
        }

        // Create FTS5 virtual table for full-text search
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

        // Create triggers to sync FTS index
        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(content, user_id, memory_id)
                VALUES (NEW.content, NEW.user_id, NEW.id);
            END
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create insert trigger".to_string(),
            details: e.to_string(),
        })?;

        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
                DELETE FROM memories_fts WHERE memory_id = OLD.id;
            END
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create delete trigger".to_string(),
            details: e.to_string(),
        })?;

        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS memories_fts_update AFTER UPDATE ON memories BEGIN
                DELETE FROM memories_fts WHERE memory_id = OLD.id;
                INSERT INTO memories_fts(content, user_id, memory_id)
                VALUES (NEW.content, NEW.user_id, NEW.id);
            END
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create update trigger".to_string(),
            details: e.to_string(),
        })?;

        debug!("Optimized schema created");
        Ok(())
    }

    /// Set batch size for bulk operations
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Get the underlying pool
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

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

    /// Run VACUUM to optimize database file
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

    /// Get database statistics
    pub async fn stats(&self) -> crate::Result<DbStats> {
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

/// Database statistics
#[derive(Debug, Clone)]
pub struct DbStats {
    /// Number of pages in the database
    pub page_count: i64,
    /// Number of pages on the freelist
    pub freelist_count: i64,
    /// Page size in bytes
    pub page_size: i64,
    /// User version
    pub user_version: i64,
    /// Total database size in bytes
    pub database_size_bytes: i64,
}

impl DbStats {
    /// Calculate fragmentation percentage
    pub fn fragmentation_percent(&self) -> f64 {
        if self.page_count == 0 {
            return 0.0;
        }
        (self.freelist_count as f64 / self.page_count as f64) * 100.0
    }

    /// Format database size as human-readable string
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

/// Query builder for complex queries
#[derive(Debug)]
pub struct QueryBuilder {
    base: String,
    conditions: Vec<String>,
    order_by: Vec<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

impl QueryBuilder {
    /// Create a new query builder
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            conditions: Vec::new(),
            order_by: Vec::new(),
            limit: None,
            offset: None,
        }
    }

    /// Add a WHERE condition
    pub fn and_where(mut self, condition: impl Into<String>) -> Self {
        self.conditions.push(condition.into());
        self
    }

    /// Add ORDER BY clause
    pub fn order_by(mut self, column: impl Into<String>, ascending: bool) -> Self {
        let dir = if ascending { "ASC" } else { "DESC" };
        self.order_by.push(format!("{} {}", column.into(), dir));
        self
    }

    /// Set LIMIT
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set OFFSET
    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Build the query string
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
}
