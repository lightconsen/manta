//! Generic RAG (Retrieval-Augmented Generation) infrastructure.
//!
//! Provides embedding, vector storage, chunking, batched embedding pipelines,
//! and hybrid search algorithms (weighted fusion, MMR, temporal decay) that
//! are fully domain-agnostic — usable for conversation memory, code
//! documentation, task instructions, or any other knowledge base without
//! pulling in the `memory` module's types.

pub mod chunk;
pub mod config;
pub mod context;
pub mod embedding;
pub mod eval;
pub mod hybrid;
pub mod ingestion;
pub mod multi_query;
pub mod pipeline;
pub mod query;
pub mod reranker;
pub mod vector_store;

#[cfg(feature = "local-embeddings")]
pub mod local_embeddings;
#[cfg(feature = "pgvector")]
pub mod pgvector_store;
#[cfg(feature = "sqlite-vec")]
pub mod sqlite_vec_store;

pub use chunk::{BatchEmbeddingProcessor, ChunkStrategy, EmbeddedChunk, TextChunker};
pub use config::{EmbeddingConfig, VectorBackend};
pub use context::{estimate_tokens, select_by_token_budget, ContextWindowConfig};
pub use embedding::{
    ApiEmbeddingProvider, CachedEmbeddingProvider, EmbeddingProvider, LocalGgufEmbeddingProvider,
};
pub use eval::{evaluate_retrieval, RetrievalMetrics, RetrievalSample};
pub use multi_query::{
    expand_query_with_llm, merge_results, MergeStrategy, MultiQueryConfig, RrfConfig,
};
pub use query::{NoopTransformer, QueryTransformer};
pub use reranker::{CohereReranker, NoopReranker, Reranker};
pub use vector_store::{MemoryVectorStore, SearchResult, VectorStore, VectorStoreStats};

#[cfg(feature = "sqlite-vec")]
pub use sqlite_vec_store::SqliteVecStore;
