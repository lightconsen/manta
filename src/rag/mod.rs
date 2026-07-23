//! Generic RAG (Retrieval-Augmented Generation) infrastructure.
//!
//! Provides embedding, vector storage, chunking, batched embedding pipelines,
//! and hybrid search algorithms (weighted fusion, MMR, temporal decay) that
//! are fully domain-agnostic — usable for conversation memory, code
//! documentation, task instructions, or any other knowledge base without
//! pulling in the `memory` module's types.

pub mod chunk;
pub mod config;
pub mod embedding;
pub mod hybrid;
pub mod pipeline;
pub mod vector_store;

#[cfg(feature = "local-embeddings")]
pub mod local_embeddings;
#[cfg(feature = "pgvector")]
pub mod pgvector_store;
#[cfg(feature = "sqlite-vec")]
pub mod sqlite_vec_store;

pub use chunk::{BatchEmbeddingProcessor, EmbeddedChunk, TextChunker};
pub use config::{EmbeddingConfig, VectorBackend};
pub use embedding::{
    ApiEmbeddingProvider, CachedEmbeddingProvider, EmbeddingProvider,
    LocalGgufEmbeddingProvider,
};
pub use vector_store::{MemoryVectorStore, SearchResult, VectorStore, VectorStoreStats};
