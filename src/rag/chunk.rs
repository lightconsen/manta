//! Document chunking and batch embedding processing.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;
use tracing::info;

use super::embedding::EmbeddingProvider;
use super::vector_store::VectorStore;

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
    /// Optional collection this chunk belongs to.
    pub collection: Option<String>,
    /// Metadata
    pub metadata: Option<serde_json::Value>,
}

/// Calculate cosine similarity between two vectors
pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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

    /// Chunk text asynchronously, offloading the CPU-bound work to the
    /// blocking pool so large documents do not stall the Tokio runtime.
    pub async fn chunk_async(&self, text: impl Into<String>) -> crate::Result<Vec<String>> {
        let chunker = self.clone();
        let text = text.into();
        spawn_blocking(move || chunker.chunk(&text))
            .await
            .map_err(|e| {
                crate::error::SyscityError::Validation(format!("chunking panicked: {}", e))
            })
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
            let chunks = self.chunker.chunk_async(content).await?;
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

            if embeddings.len() != batch.len() {
                return Err(crate::error::SyscityError::Validation(format!(
                    "Embedding provider returned {} embeddings for {} chunks",
                    embeddings.len(),
                    batch.len()
                )));
            }

            for (idx, (doc_id, text, pos, total)) in batch.iter().enumerate() {
                if let Some(embedding) = embeddings.get(idx) {
                    embedded_chunks.push(EmbeddedChunk {
                        id: format!("{}-{}-{}", chunk_id_base, batch_idx, idx),
                        source_id: doc_id.clone(),
                        text: text.clone(),
                        embedding: embedding.clone(),
                        position: *pos,
                        total_chunks: *total,
                        collection: None,
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

    #[test]
    fn test_text_chunker_empty() {
        let chunker = TextChunker::new(5, 2);
        let chunks = chunker.chunk("");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_text_chunker_exact_size() {
        let chunker = TextChunker::new(3, 1);
        let text = "one two three";
        let chunks = chunker.chunk(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "one two three");
    }

    #[test]
    fn test_text_chunker_overlap_produces_multiple() {
        let chunker = TextChunker::new(3, 1);
        let text = "a b c d e f g";
        let chunks = chunker.chunk(text);
        assert!(chunks.len() > 1);
        // First chunk should start with 'a'
        assert!(chunks[0].starts_with('a'));
        // Overlap: second chunk should share some words with first
        assert!(chunks[1].contains('c'));
    }

    #[test]
    fn test_cosine_similarity_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn test_cosine_similarity_mismatched_lengths() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_embedded_chunk_creation() {
        let chunk = EmbeddedChunk {
            id: "id1".to_string(),
            source_id: "src1".to_string(),
            text: "hello".to_string(),
            embedding: vec![0.1, 0.2],
            position: 3,
            total_chunks: 5,
            collection: None,
            metadata: Some(serde_json::json!({"key": "val"})),
        };
        assert_eq!(chunk.id, "id1");
        assert_eq!(chunk.position, 3);
        assert_eq!(chunk.total_chunks, 5);
    }
}
