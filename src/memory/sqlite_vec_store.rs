//! SQLite + sqlite-vec extension vector store backend.

#![allow(unsafe_code)]

use std::os::raw::{c_char, c_int};
use std::sync::OnceLock;

use async_trait::async_trait;
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};
use tracing::info;

use super::vector::{EmbeddedChunk, VectorStore, VectorStoreStats};

/// SQLite-backed vector store using the native `sqlite-vec` extension.
#[derive(Debug, Clone)]
pub struct SqliteVecStore {
    pool: Pool<Sqlite>,
    dimension: usize,
}

/// Register the sqlite-vec extension as a SQLite auto-extension so that every
/// new connection created by SQLx (which shares the same SQLite library) has
/// the extension available.
mod sqlite_vec_ext {
    use super::*;

    type AutoExtension = unsafe extern "C" fn(
        *mut libsqlite3_sys::sqlite3,
        *mut *mut c_char,
        *const libsqlite3_sys::sqlite3_api_routines,
    ) -> c_int;

    fn register() -> Result<(), String> {
        static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
        RESULT
            .get_or_init(|| {
                let init: AutoExtension =
                    unsafe { std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ()) };
                let rc = unsafe { libsqlite3_sys::sqlite3_auto_extension(Some(init)) };
                if rc == libsqlite3_sys::SQLITE_OK {
                    Ok(())
                } else {
                    Err(format!("sqlite3_auto_extension returned error code {}", rc))
                }
            })
            .clone()
    }

    pub fn ensure_registered() -> crate::Result<()> {
        register().map_err(|details| crate::error::SyscityError::Storage {
            context: "Failed to register sqlite-vec extension".to_string(),
            details,
        })
    }
}

impl SqliteVecStore {
    /// Create a new sqlite-vec-backed store.
    pub async fn new(path: &str, dimension: usize) -> crate::Result<Self> {
        sqlite_vec_ext::ensure_registered()?;
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(path)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to connect to SQLite at {}", path),
                details: e.to_string(),
            })?;

        let create_sql = format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(
                embedding float[{}] distance_metric=cosine,
                +id text,
                +source_id text,
                +text text,
                +position integer,
                +total_chunks integer,
                +metadata text
            )",
            dimension
        );
        sqlx::query(&create_sql).execute(&pool).await.map_err(|e| {
            crate::error::SyscityError::Storage {
                context: "Failed to create sqlite-vec virtual table".to_string(),
                details: e.to_string(),
            }
        })?;

        info!("SqliteVecStore initialized at {} (dim={})", path, dimension);
        Ok(Self { pool, dimension })
    }

    /// Create an in-memory sqlite-vec store (for testing).
    pub async fn new_in_memory(dimension: usize) -> crate::Result<Self> {
        Self::new("sqlite::memory:", dimension).await
    }
}

fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[async_trait]
impl VectorStore for SqliteVecStore {
    async fn store_chunk(&self, chunk: EmbeddedChunk) -> crate::Result<()> {
        let embedding_bytes = embedding_to_bytes(&chunk.embedding);
        let metadata_json = chunk
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to serialize metadata".to_string(),
                details: e.to_string(),
            })?;

        sqlx::query(
            "INSERT OR REPLACE INTO vec_chunks
             (id, source_id, text, embedding, position, total_chunks, metadata)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&chunk.id)
        .bind(&chunk.source_id)
        .bind(&chunk.text)
        .bind(&embedding_bytes)
        .bind(chunk.position as i64)
        .bind(chunk.total_chunks as i64)
        .bind(metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to store sqlite-vec chunk".to_string(),
            details: e.to_string(),
        })?;
        Ok(())
    }

    async fn search_similar(
        &self,
        query_embedding: &[f32],
        limit: usize,
        threshold: f32,
    ) -> crate::Result<Vec<(EmbeddedChunk, f32)>> {
        let query_bytes = embedding_to_bytes(query_embedding);
        let max_distance = 1.0f64 - threshold as f64;

        let rows = sqlx::query(
            "SELECT rowid, id, source_id, text, position, total_chunks, metadata, distance
             FROM vec_chunks
             WHERE embedding MATCH ? AND distance <= ?
             ORDER BY distance
             LIMIT ?",
        )
        .bind(&query_bytes)
        .bind(max_distance)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to search sqlite-vec".to_string(),
            details: e.to_string(),
        })?;

        let mut results = Vec::new();
        for row in rows {
            let distance: f64 =
                row.try_get("distance")
                    .map_err(|e| crate::error::SyscityError::Storage {
                        context: "Failed to read sqlite-vec distance".to_string(),
                        details: e.to_string(),
                    })?;
            let metadata: Option<String> = row.try_get("metadata").ok();
            let chunk = EmbeddedChunk {
                id: row
                    .try_get("id")
                    .map_err(|e| crate::error::SyscityError::Storage {
                        context: "Failed to read sqlite-vec id".to_string(),
                        details: e.to_string(),
                    })?,
                source_id: row.try_get("source_id").map_err(|e| {
                    crate::error::SyscityError::Storage {
                        context: "Failed to read sqlite-vec source_id".to_string(),
                        details: e.to_string(),
                    }
                })?,
                text: row
                    .try_get("text")
                    .map_err(|e| crate::error::SyscityError::Storage {
                        context: "Failed to read sqlite-vec text".to_string(),
                        details: e.to_string(),
                    })?,
                embedding: Vec::new(),
                position: row.try_get::<i64, _>("position").map_err(|e| {
                    crate::error::SyscityError::Storage {
                        context: "Failed to read sqlite-vec position".to_string(),
                        details: e.to_string(),
                    }
                })? as usize,
                total_chunks: row.try_get::<i64, _>("total_chunks").map_err(|e| {
                    crate::error::SyscityError::Storage {
                        context: "Failed to read sqlite-vec total_chunks".to_string(),
                        details: e.to_string(),
                    }
                })? as usize,
                metadata: metadata.and_then(|m| serde_json::from_str(&m).ok()),
            };
            results.push((chunk, (1.0f64 - distance) as f32));
        }
        Ok(results)
    }

    async fn delete_by_source(&self, source_id: &str) -> crate::Result<usize> {
        let result = sqlx::query("DELETE FROM vec_chunks WHERE source_id = ?")
            .bind(source_id)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to delete by source".to_string(),
                details: e.to_string(),
            })?;
        Ok(result.rows_affected() as usize)
    }

    async fn stats(&self) -> crate::Result<VectorStoreStats> {
        let total_vectors: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM vec_chunks")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to count vectors".to_string(),
                details: e.to_string(),
            })?;
        let total_sources: (i64,) =
            sqlx::query_as("SELECT COUNT(DISTINCT source_id) FROM vec_chunks")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| crate::error::SyscityError::Storage {
                    context: "Failed to count sources".to_string(),
                    details: e.to_string(),
                })?;
        Ok(VectorStoreStats {
            total_vectors: total_vectors.0 as usize,
            total_sources: total_sources.0 as usize,
            dimension: self.dimension,
        })
    }

    async fn clear(&self) -> crate::Result<()> {
        sqlx::query("DELETE FROM vec_chunks")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to clear sqlite-vec table".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::vector::EmbeddedChunk;
    use super::*;

    #[tokio::test]
    async fn test_sqlite_vec_store_store_and_search() -> crate::Result<()> {
        let store = SqliteVecStore::new_in_memory(3).await?;
        let chunk = EmbeddedChunk {
            id: "c1".to_string(),
            source_id: "doc1".to_string(),
            text: "hello world".to_string(),
            embedding: vec![1.0, 0.0, 0.0],
            position: 0,
            total_chunks: 1,
            metadata: None,
        };
        store.store_chunk(chunk).await?;

        let results = store.search_similar(&[1.0, 0.0, 0.0], 5, 0.0).await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.id, "c1");
        assert!((results[0].1 - 1.0).abs() < 0.001);

        let results = store.search_similar(&[0.0, 1.0, 0.0], 5, 0.5).await?;
        assert!(results.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_sqlite_vec_store_delete_by_source() -> crate::Result<()> {
        let store = SqliteVecStore::new_in_memory(2).await?;
        store
            .store_chunk(EmbeddedChunk {
                id: "c1".to_string(),
                source_id: "doc-a".to_string(),
                text: "a".to_string(),
                embedding: vec![1.0, 0.0],
                position: 0,
                total_chunks: 2,
                metadata: None,
            })
            .await?;
        store
            .store_chunk(EmbeddedChunk {
                id: "c2".to_string(),
                source_id: "doc-a".to_string(),
                text: "b".to_string(),
                embedding: vec![0.0, 1.0],
                position: 1,
                total_chunks: 2,
                metadata: None,
            })
            .await?;
        store
            .store_chunk(EmbeddedChunk {
                id: "c3".to_string(),
                source_id: "doc-b".to_string(),
                text: "c".to_string(),
                embedding: vec![1.0, 1.0],
                position: 0,
                total_chunks: 1,
                metadata: None,
            })
            .await?;

        let deleted = store.delete_by_source("doc-a").await?;
        assert_eq!(deleted, 2);

        let stats = store.stats().await?;
        assert_eq!(stats.total_vectors, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_sqlite_vec_store_stats_and_clear() -> crate::Result<()> {
        let store = SqliteVecStore::new_in_memory(4).await?;
        store
            .store_chunk(EmbeddedChunk {
                id: "c1".to_string(),
                source_id: "s1".to_string(),
                text: "a".to_string(),
                embedding: vec![0.0; 4],
                position: 0,
                total_chunks: 1,
                metadata: None,
            })
            .await?;
        store
            .store_chunk(EmbeddedChunk {
                id: "c2".to_string(),
                source_id: "s2".to_string(),
                text: "b".to_string(),
                embedding: vec![0.0; 4],
                position: 0,
                total_chunks: 1,
                metadata: None,
            })
            .await?;

        let stats = store.stats().await?;
        assert_eq!(stats.total_vectors, 2);
        assert_eq!(stats.total_sources, 2);
        assert_eq!(stats.dimension, 4);

        store.clear().await?;
        let stats = store.stats().await?;
        assert_eq!(stats.total_vectors, 0);
        Ok(())
    }
}
