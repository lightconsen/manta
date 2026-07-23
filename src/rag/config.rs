//! Configuration types for vector embeddings and storage backends.

use serde::{Deserialize, Serialize};

/// Configuration for vector database backend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorBackend {
    /// SQLite with the sqlite-vec extension (native vector search).
    /// This is the default persistent backend.
    #[cfg(feature = "sqlite-vec")]
    SqliteVec { path: String },
    /// In-memory storage (for testing/small datasets)
    Memory,
    /// PostgreSQL with pgvector
    #[cfg(feature = "pgvector")]
    Postgres { url: String, table: String },
}

fn default_vector_db_path() -> String {
    format!(
        "sqlite:///{}",
        crate::dirs::syscity_dir()
            .join("data")
            .join("syscity.db")
            .display()
    )
}

impl Default for VectorBackend {
    fn default() -> Self {
        #[cfg(feature = "sqlite-vec")]
        {
            VectorBackend::SqliteVec { path: default_vector_db_path() }
        }
        #[cfg(not(feature = "sqlite-vec"))]
        {
            VectorBackend::Memory
        }
    }
}

/// Configuration for embedding model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model name (e.g., "BAAI/bge-small-en", "nomic-ai/nomic-embed-text-v1")
    pub model: String,
    /// Maximum chunk size for text splitting
    pub chunk_size: usize,
    /// Chunk overlap for sliding window
    pub chunk_overlap: usize,
    /// Batch size for embedding generation
    pub batch_size: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "BAAI/bge-small-en".to_string(),
            chunk_size: 512,
            chunk_overlap: 50,
            batch_size: 32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_backend_default() {
        #[cfg(feature = "sqlite-vec")]
        assert!(matches!(VectorBackend::default(), VectorBackend::SqliteVec { .. }));
        #[cfg(not(feature = "sqlite-vec"))]
        assert!(matches!(VectorBackend::default(), VectorBackend::Memory));
    }

    #[test]
    fn test_embedding_config_default() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.model, "BAAI/bge-small-en");
        assert_eq!(config.chunk_size, 512);
        assert_eq!(config.chunk_overlap, 50);
        assert_eq!(config.batch_size, 32);
    }
}
