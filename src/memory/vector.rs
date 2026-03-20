//! Vector Database and Embeddings System
//!
//! Provides semantic search capabilities similar to OpenClaw's QMD/LanceDB:
//! - Embedding generation using fastembed (local) or API providers
//! - Vector storage with SQLite vec extension or in-memory
//! - Semantic similarity search with cosine similarity
//! - Batch processing for efficient embedding generation

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::{Memory, MemoryId, MemoryQuery};

/// Configuration for vector database backend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorBackend {
    /// SQLite with vector extension
    Sqlite { path: String },
    /// In-memory storage (for testing/small datasets)
    Memory,
    /// QMD-style: query-model database (future)
    #[cfg(feature = "pgvector")]
    Postgres { url: String, table: String },
}

impl Default for VectorBackend {
    fn default() -> Self {
        VectorBackend::Memory
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

/// A document chunk with its embedding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedChunk {
    /// Unique identifier
    pub id: String,
    /// Original document/content ID
    pub source_id: String,
    /// The text chunk
    pub text: String,
    /// Embedding vector
    pub embedding: Vec<f32>,
    /// Chunk position in original document
    pub position: usize,
    /// Total chunks for this source
    pub total_chunks: usize,
    /// Metadata
    pub metadata: Option<serde_json::Value>,
}

/// Trait for embedding providers
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Get the model name
    fn model_name(&self) -> &str;

    /// Get the embedding dimension
    fn dimension(&self) -> usize;

    /// Generate embeddings for texts (batch)
    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>>;

    /// Generate embedding for single text
    async fn embed(&self, text: &str) -> crate::Result<Vec<f32>> {
        let mut results = self.embed_batch(&[text.to_string()]).await?;
        Ok(results.pop().unwrap_or_default())
    }
}

/// Blanket impl so `Arc<dyn EmbeddingProvider>` can be passed where a concrete
/// `EmbeddingProvider` is expected (e.g. as the inner of `CachedEmbeddingProvider`).
#[async_trait]
impl EmbeddingProvider for Arc<dyn EmbeddingProvider> {
    fn model_name(&self) -> &str {
        (**self).model_name()
    }

    fn dimension(&self) -> usize {
        (**self).dimension()
    }

    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        (**self).embed_batch(texts).await
    }
}

/// API-based embedding provider (OpenAI, etc.)
pub struct ApiEmbeddingProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    dimension: usize,
}

/// Re-export the LocalEmbeddingProvider from local_embeddings module
#[cfg(feature = "local-embeddings")]
pub use super::local_embeddings::LocalEmbeddingProvider as LocalGgufEmbeddingProvider;

/// Stub when local-embeddings feature is disabled
#[cfg(not(feature = "local-embeddings"))]
pub struct LocalGgufEmbeddingProvider;

#[cfg(not(feature = "local-embeddings"))]
impl LocalGgufEmbeddingProvider {
    /// Create stub
    pub async fn create(_source: (), _dimension: usize) -> Self {
        Self
    }

    /// FTS-only stub
    pub fn fts_only(_reason: impl Into<String>) -> Self {
        Self
    }

    /// Always returns true for stub
    pub fn is_fts_only(&self) -> bool {
        true
    }

    /// Returns the reason for FTS-only mode
    pub fn fts_reason(&self) -> Option<&str> {
        Some("'local-embeddings' feature not enabled")
    }

    /// Always returns error
    pub async fn embed_batch(&self, _texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        Err(crate::error::MantaError::Validation(
            "Local GGUF embeddings require 'local-embeddings' feature. Install with: cargo build --features local-embeddings".to_string()
        ))
    }

    /// Returns stub name
    pub fn model_name(&self) -> &str {
        "disabled"
    }

    /// Returns 0
    pub fn dimension(&self) -> usize {
        0
    }
}

#[cfg(not(feature = "local-embeddings"))]
#[async_trait::async_trait]
impl EmbeddingProvider for LocalGgufEmbeddingProvider {
    fn model_name(&self) -> &str {
        self.model_name()
    }

    fn dimension(&self) -> usize {
        self.dimension()
    }

    async fn embed_batch(&self, _texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        self.embed_batch(_texts).await
    }
}

impl ApiEmbeddingProvider {
    /// Create a new API embedding provider
    pub fn new(api_key: String, model: String, dimension: usize) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: "https://api.openai.com/v1".to_string(),
            model,
            dimension,
        }
    }

    /// Set custom base URL (for Azure, etc.)
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }
}

#[async_trait]
impl EmbeddingProvider for ApiEmbeddingProvider {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        #[derive(Debug, Serialize)]
        struct Request {
            model: String,
            input: Vec<String>,
        }

        #[derive(Debug, Deserialize)]
        struct EmbeddingResponse {
            data: Vec<EmbeddingData>,
        }

        #[derive(Debug, Deserialize)]
        struct EmbeddingData {
            embedding: Vec<f32>,
            index: usize,
        }

        let request = Request {
            model: self.model.clone(),
            input: texts.to_vec(),
        };

        let response: EmbeddingResponse = self
            .client
            .post(format!("{}/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Embedding API request failed".to_string(),
                cause: Some(Box::new(e)),
            })?
            .json()
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: "Invalid embedding response".to_string(),
                cause: Some(Box::new(e)),
            })?;

        // Sort by index to maintain order
        let mut embeddings: Vec<(usize, Vec<f32>)> = response
            .data
            .into_iter()
            .map(|d| (d.index, d.embedding))
            .collect();
        embeddings.sort_by_key(|(idx, _)| *idx);

        Ok(embeddings.into_iter().map(|(_, emb)| emb).collect())
    }
}

// ── Embedding dedup cache ─────────────────────────────────────────────────────

/// In-memory SHA-256 content-dedup cache for embedding vectors.
///
/// Wraps any [`EmbeddingProvider`] and skips API calls for texts whose SHA-256
/// hash has already been cached.  The cache is bounded to `max_entries`; when
/// full, the oldest inserted entry is evicted (simple FIFO).
///
/// # Example
///
/// ```rust,no_run
/// # use std::sync::Arc;
/// # use manta::memory::vector::{ApiEmbeddingProvider, CachedEmbeddingProvider};
/// let inner = ApiEmbeddingProvider::new("key".into(), "text-embedding-3-small".into(), 1536);
/// let cached = CachedEmbeddingProvider::new(inner, 10_000);
/// ```
pub struct CachedEmbeddingProvider<P: EmbeddingProvider> {
    inner: P,
    /// SHA-256 hex → embedding vector.
    cache: RwLock<std::collections::HashMap<String, Vec<f32>>>,
    /// Insertion-order keys for FIFO eviction.
    order: RwLock<std::collections::VecDeque<String>>,
    max_entries: usize,
}

impl<P: EmbeddingProvider> CachedEmbeddingProvider<P> {
    /// Wrap `provider` with a FIFO dedup cache capped at `max_entries`.
    pub fn new(provider: P, max_entries: usize) -> Self {
        Self {
            inner: provider,
            cache: RwLock::new(std::collections::HashMap::new()),
            order: RwLock::new(std::collections::VecDeque::new()),
            max_entries,
        }
    }

    /// SHA-256 hex digest of `text` used as the cache key.
    fn sha256_key(text: &str) -> String {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(text.as_bytes());
        format!("{:x}", hash)
    }

    /// Current number of cached entries.
    pub async fn cache_size(&self) -> usize {
        self.cache.read().await.len()
    }

    /// Remove all cached entries.
    pub async fn clear_cache(&self) {
        self.cache.write().await.clear();
        self.order.write().await.clear();
    }
}

#[async_trait]
impl<P: EmbeddingProvider + Send + Sync> EmbeddingProvider for CachedEmbeddingProvider<P> {
    fn model_name(&self) -> &str {
        self.inner.model_name()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        let mut result: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut miss_indices: Vec<usize> = Vec::new();
        let mut miss_texts: Vec<String> = Vec::new();

        // Cache-hit pass.
        {
            let cache = self.cache.read().await;
            for (i, text) in texts.iter().enumerate() {
                let key = Self::sha256_key(text);
                if let Some(emb) = cache.get(&key) {
                    result[i] = Some(emb.clone());
                } else {
                    miss_indices.push(i);
                    miss_texts.push(text.clone());
                }
            }
        }

        if miss_texts.is_empty() {
            return Ok(result.into_iter().flatten().collect());
        }

        // Fetch missing embeddings from the inner provider.
        let fetched = self.inner.embed_batch(&miss_texts).await?;

        // Store fetched embeddings in cache, evicting oldest if full.
        {
            let mut cache = self.cache.write().await;
            let mut order = self.order.write().await;

            for (text, embedding) in miss_texts.iter().zip(fetched.iter()) {
                let key = Self::sha256_key(text);
                if !cache.contains_key(&key) {
                    // Evict oldest if at capacity.
                    if cache.len() >= self.max_entries {
                        if let Some(oldest) = order.pop_front() {
                            cache.remove(&oldest);
                        }
                    }
                    cache.insert(key.clone(), embedding.clone());
                    order.push_back(key);
                }
            }
        }

        // Merge fetched embeddings back into result.
        for (local_idx, global_idx) in miss_indices.into_iter().enumerate() {
            result[global_idx] = Some(fetched[local_idx].clone());
        }

        Ok(result.into_iter().flatten().collect())
    }
}

/// Vector storage trait
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Store a chunk with its embedding
    async fn store_chunk(&self, chunk: EmbeddedChunk) -> crate::Result<()>;

    /// Store multiple chunks
    async fn store_chunks(&self, chunks: Vec<EmbeddedChunk>) -> crate::Result<()> {
        for chunk in chunks {
            self.store_chunk(chunk).await?;
        }
        Ok(())
    }

    /// Search for similar chunks
    async fn search_similar(
        &self,
        query_embedding: &[f32],
        limit: usize,
        threshold: f32,
    ) -> crate::Result<Vec<(EmbeddedChunk, f32)>>;

    /// Delete chunks by source ID
    async fn delete_by_source(&self, source_id: &str) -> crate::Result<usize>;

    /// Get stats about the store
    async fn stats(&self) -> crate::Result<VectorStoreStats>;

    /// Clear all data
    async fn clear(&self) -> crate::Result<()>;
}

/// Statistics about the vector store
#[derive(Debug, Clone, Default)]
pub struct VectorStoreStats {
    pub total_vectors: usize,
    pub total_sources: usize,
    pub dimension: usize,
}

/// In-memory vector store (for testing/small datasets)
pub struct MemoryVectorStore {
    chunks: RwLock<HashMap<String, EmbeddedChunk>>,
    dimension: usize,
}

impl MemoryVectorStore {
    pub fn new(dimension: usize) -> Self {
        Self {
            chunks: RwLock::new(HashMap::new()),
            dimension,
        }
    }
}

#[async_trait]
impl VectorStore for MemoryVectorStore {
    async fn store_chunk(&self, chunk: EmbeddedChunk) -> crate::Result<()> {
        let mut chunks = self.chunks.write().await;
        chunks.insert(chunk.id.clone(), chunk);
        Ok(())
    }

    async fn search_similar(
        &self,
        query_embedding: &[f32],
        limit: usize,
        threshold: f32,
    ) -> crate::Result<Vec<(EmbeddedChunk, f32)>> {
        let chunks = self.chunks.read().await;

        let mut results: Vec<(EmbeddedChunk, f32)> = chunks
            .values()
            .filter_map(|chunk| {
                let similarity = cosine_similarity(query_embedding, &chunk.embedding);
                if similarity >= threshold {
                    Some((chunk.clone(), similarity))
                } else {
                    None
                }
            })
            .collect();

        // Sort by similarity (descending)
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        results.truncate(limit);

        Ok(results)
    }

    async fn delete_by_source(&self, source_id: &str) -> crate::Result<usize> {
        let mut chunks = self.chunks.write().await;
        let to_remove: Vec<String> = chunks
            .values()
            .filter(|c| c.source_id == source_id)
            .map(|c| c.id.clone())
            .collect();

        let count = to_remove.len();
        for id in to_remove {
            chunks.remove(&id);
        }

        Ok(count)
    }

    async fn stats(&self) -> crate::Result<VectorStoreStats> {
        let chunks = self.chunks.read().await;
        let sources: std::collections::HashSet<String> =
            chunks.values().map(|c| c.source_id.clone()).collect();

        Ok(VectorStoreStats {
            total_vectors: chunks.len(),
            total_sources: sources.len(),
            dimension: self.dimension,
        })
    }

    async fn clear(&self) -> crate::Result<()> {
        let mut chunks = self.chunks.write().await;
        chunks.clear();
        Ok(())
    }
}

/// Calculate cosine similarity between two vectors
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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

/// Text chunking for long documents
#[derive(Debug, Clone)]
pub struct TextChunker {
    chunk_size: usize,
    chunk_overlap: usize,
}

impl TextChunker {
    pub fn new(chunk_size: usize, chunk_overlap: usize) -> Self {
        Self { chunk_size, chunk_overlap }
    }

    /// Chunk text into overlapping segments
    pub fn chunk(&self, text: &str) -> Vec<String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let mut chunks = Vec::new();
        let mut start = 0;

        while start < words.len() {
            let end = (start + self.chunk_size).min(words.len());
            let chunk = words[start..end].join(" ");
            chunks.push(chunk);

            if end >= words.len() {
                break;
            }

            start += self.chunk_size - self.chunk_overlap;
        }

        chunks
    }
}

/// Batch processor for efficient embedding generation
pub struct BatchEmbeddingProcessor {
    provider: Arc<dyn EmbeddingProvider>,
    chunker: TextChunker,
    batch_size: usize,
}

impl BatchEmbeddingProcessor {
    pub fn new(
        provider: Arc<dyn EmbeddingProvider>,
        chunker: TextChunker,
        batch_size: usize,
    ) -> Self {
        Self { provider, chunker, batch_size }
    }

    /// Process documents and store embeddings
    pub async fn process_documents(
        &self,
        documents: Vec<(String, String)>, // (id, content)
        store: &dyn VectorStore,
    ) -> crate::Result<Vec<EmbeddedChunk>> {
        let mut all_chunks = Vec::new();

        // Chunk all documents
        for (doc_id, content) in &documents {
            let chunks = self.chunker.chunk(content);
            let total = chunks.len();

            for (pos, text) in chunks.into_iter().enumerate() {
                all_chunks.push((doc_id.clone(), text, pos, total));
            }
        }

        // Process in batches
        let mut embedded_chunks = Vec::new();
        let chunk_id_base = uuid::Uuid::new_v4().to_string();

        for (batch_idx, batch) in all_chunks.chunks(self.batch_size).enumerate() {
            let texts: Vec<String> = batch.iter().map(|(_, text, _, _)| text.clone()).collect();
            let embeddings = self.provider.embed_batch(&texts).await?;

            for (idx, (doc_id, text, pos, total)) in batch.iter().enumerate() {
                if let Some(embedding) = embeddings.get(idx) {
                    embedded_chunks.push(EmbeddedChunk {
                        id: format!("{}-{}-{}", chunk_id_base, batch_idx, idx),
                        source_id: doc_id.clone(),
                        text: text.clone(),
                        embedding: embedding.clone(),
                        position: *pos,
                        total_chunks: *total,
                        metadata: None,
                    });
                }
            }
        }

        // Store all chunks
        store.store_chunks(embedded_chunks.clone()).await?;

        info!("Processed {} documents into {} chunks", documents.len(), embedded_chunks.len());

        Ok(embedded_chunks)
    }
}

/// High-level vector memory service
pub struct VectorMemoryService {
    embedding_provider: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStore>,
    chunker: TextChunker,
    batch_processor: BatchEmbeddingProcessor,
}

impl VectorMemoryService {
    /// Create a new vector memory service
    pub fn new(
        embedding_provider: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn VectorStore>,
        config: &EmbeddingConfig,
    ) -> Self {
        let chunker = TextChunker::new(config.chunk_size, config.chunk_overlap);
        let batch_processor = BatchEmbeddingProcessor::new(
            embedding_provider.clone(),
            chunker.clone(),
            config.batch_size,
        );

        Self {
            embedding_provider,
            vector_store,
            chunker,
            batch_processor,
        }
    }

    /// Store a memory with automatic chunking and embedding
    pub async fn store_memory(&self, memory: &Memory) -> crate::Result<Vec<EmbeddedChunk>> {
        let chunks = self.chunker.chunk(&memory.content);
        let total = chunks.len();

        let mut embedded_chunks = Vec::new();
        let embeddings = self.embedding_provider.embed_batch(&chunks).await?;

        for (pos, (text, embedding)) in chunks.into_iter().zip(embeddings.into_iter()).enumerate() {
            embedded_chunks.push(EmbeddedChunk {
                id: format!("{}-{}", memory.id, pos),
                source_id: memory.id.to_string(),
                text,
                embedding,
                position: pos,
                total_chunks: total,
                metadata: memory.metadata.clone(),
            });
        }

        self.vector_store
            .store_chunks(embedded_chunks.clone())
            .await?;

        Ok(embedded_chunks)
    }

    /// Search memories semantically
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
        threshold: f32,
    ) -> crate::Result<Vec<(EmbeddedChunk, f32)>> {
        let query_embedding = self.embedding_provider.embed(query).await?;
        self.vector_store
            .search_similar(&query_embedding, limit, threshold)
            .await
    }

    /// Delete memory embeddings
    pub async fn delete_memory(&self, memory_id: &MemoryId) -> crate::Result<usize> {
        self.vector_store
            .delete_by_source(&memory_id.to_string())
            .await
    }

    /// Get stats
    pub async fn stats(&self) -> crate::Result<VectorStoreStats> {
        self.vector_store.stats().await
    }

    /// Search memories in a specific collection (simplified API for gateway)
    pub async fn search_collection(
        &self,
        query: &str,
        limit: usize,
        _collection: &str,
    ) -> crate::Result<Vec<SearchResult>> {
        let query_embedding = self.embedding_provider.embed(query).await?;
        let results = self
            .vector_store
            .search_similar(&query_embedding, limit, 0.7)
            .await?;

        Ok(results
            .into_iter()
            .map(|(chunk, score)| SearchResult {
                id: chunk.id,
                content: chunk.text,
                score,
                metadata: chunk.metadata,
            })
            .collect())
    }

    /// Add content to a collection (simplified API for gateway)
    pub async fn add_to_collection(
        &self,
        content: &str,
        metadata: Option<serde_json::Value>,
        collection: &str,
    ) -> crate::Result<String> {
        let doc_id = uuid::Uuid::new_v4().to_string();
        let chunks = self.chunker.chunk(content);
        let total = chunks.len();

        let embeddings = self.embedding_provider.embed_batch(&chunks).await?;

        let embedded_chunks: Vec<EmbeddedChunk> = chunks
            .into_iter()
            .zip(embeddings.into_iter())
            .enumerate()
            .map(|(pos, (text, embedding))| EmbeddedChunk {
                id: format!("{}-{}-{}", doc_id, collection, pos),
                source_id: doc_id.clone(),
                text,
                embedding,
                position: pos,
                total_chunks: total,
                metadata: metadata.clone(),
            })
            .collect();

        self.vector_store.store_chunks(embedded_chunks).await?;

        Ok(doc_id)
    }

    /// List available collections
    pub fn list_collections(&self) -> Vec<String> {
        vec!["default".to_string()]
    }
}

/// Search result for API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub content: String,
    pub score: f32,
    pub metadata: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_chunker() {
        let chunker = TextChunker::new(5, 2);
        let text = "This is a test of the chunking system for long documents";
        let chunks = chunker.chunk(text);

        assert!(!chunks.is_empty());
        assert!(chunks[0].contains("This"));
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let c = vec![1.0, 0.0, 0.0];

        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 0.001);
        assert!((cosine_similarity(&a, &c) - 1.0).abs() < 0.001);
    }

    // ── CachedEmbeddingProvider tests ────────────────────────────────────────

    /// Minimal stub that counts embed_batch calls.
    struct CountingProvider {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        dim: usize,
    }

    #[async_trait]
    impl EmbeddingProvider for CountingProvider {
        fn model_name(&self) -> &str {
            "stub"
        }
        fn dimension(&self) -> usize {
            self.dim
        }
        async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(texts.iter().map(|_| vec![0.0_f32; self.dim]).collect())
        }
    }

    #[tokio::test]
    async fn test_cached_embedding_hit() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = CountingProvider { calls: calls.clone(), dim: 4 };
        let cached = CachedEmbeddingProvider::new(provider, 100);

        let texts = vec!["hello world".to_string()];
        let _ = cached.embed_batch(&texts).await.unwrap();
        let _ = cached.embed_batch(&texts).await.unwrap();

        // Second call should be served from cache → only 1 actual call to inner.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(cached.cache_size().await, 1);
    }

    #[tokio::test]
    async fn test_cached_embedding_miss_different_text() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = CountingProvider { calls: calls.clone(), dim: 4 };
        let cached = CachedEmbeddingProvider::new(provider, 100);

        let _ = cached.embed_batch(&["text_a".to_string()]).await.unwrap();
        let _ = cached.embed_batch(&["text_b".to_string()]).await.unwrap();

        // Each unique text is a cache miss.
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(cached.cache_size().await, 2);
    }

    #[tokio::test]
    async fn test_cached_embedding_eviction() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = CountingProvider { calls: calls.clone(), dim: 2 };
        let cached = CachedEmbeddingProvider::new(provider, 2); // cap = 2

        let _ = cached.embed_batch(&["a".to_string()]).await.unwrap();
        let _ = cached.embed_batch(&["b".to_string()]).await.unwrap();
        // Full: inserting "c" should evict "a".
        let _ = cached.embed_batch(&["c".to_string()]).await.unwrap();

        assert_eq!(cached.cache_size().await, 2);
    }

    #[tokio::test]
    async fn test_cached_embedding_clear() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = CountingProvider { calls: calls.clone(), dim: 2 };
        let cached = CachedEmbeddingProvider::new(provider, 100);

        let _ = cached.embed_batch(&["hello".to_string()]).await.unwrap();
        assert_eq!(cached.cache_size().await, 1);

        cached.clear_cache().await;
        assert_eq!(cached.cache_size().await, 0);
    }
}
