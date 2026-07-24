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

/// Chunking strategy for text splitting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChunkStrategy {
    /// Fixed-size word-level sliding window with overlap.
    Fixed {
        chunk_size: usize,
        chunk_overlap: usize,
    },
    /// Recursive splitting: try separators in priority order (coarse to fine).
    /// When a piece exceeds `chunk_size`, it is recursively split using the
    /// next separator.  Falls back to word-level fixed chunking when all
    /// separators are exhausted.
    Recursive {
        chunk_size: usize,
        /// Separator strings ordered from coarsest to finest.
        /// `None` uses the default set: `["\n\n", "\n", ". ", " "]`.
        #[serde(skip_serializing_if = "Option::is_none")]
        separators: Option<Vec<String>>,
    },
}

impl Default for ChunkStrategy {
    fn default() -> Self {
        Self::Recursive {
            chunk_size: 512,
            separators: None,
        }
    }
}

pub(crate) fn default_recursive_separators() -> Vec<String> {
    vec![
        "\n\n".to_string(),
        "\n".to_string(),
        ". ".to_string(),
        " ".to_string(),
    ]
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
    strategy: ChunkStrategy,
}

impl TextChunker {
    /// Create a chunker with the default `Fixed` strategy.
    pub fn new(chunk_size: usize, chunk_overlap: usize) -> Self {
        Self {
            strategy: ChunkStrategy::Fixed { chunk_size, chunk_overlap },
        }
    }

    /// Create a chunker with a specific strategy.
    pub fn with_strategy(strategy: ChunkStrategy) -> Self {
        Self { strategy }
    }

    /// Return a reference to the current strategy.
    pub fn strategy(&self) -> &ChunkStrategy {
        &self.strategy
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

    /// Chunk text according to the configured strategy.
    pub fn chunk(&self, text: &str) -> Vec<String> {
        match &self.strategy {
            ChunkStrategy::Fixed { chunk_size, chunk_overlap } => {
                Self::chunk_fixed(text, *chunk_size, *chunk_overlap)
            }
            ChunkStrategy::Recursive { chunk_size, separators } => {
                let seps = separators
                    .clone()
                    .unwrap_or_else(default_recursive_separators);
                Self::chunk_recursive(text, *chunk_size, &seps)
            }
        }
    }

    // ── Fixed strategy ───────────────────────────────────────────────────────

    /// Word-level sliding-window chunking with overlap.
    ///
    /// When `chunk_overlap >= chunk_size` the step is clamped to 1 so the
    /// loop makes progress instead of running forever.
    fn chunk_fixed(text: &str, chunk_size: usize, chunk_overlap: usize) -> Vec<String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() || chunk_size == 0 {
            return Vec::new();
        }
        let mut chunks = Vec::new();
        let step = if chunk_overlap >= chunk_size { 1 } else { chunk_size - chunk_overlap };

        let mut start = 0;
        while start < words.len() {
            let end = (start + chunk_size).min(words.len());
            chunks.push(words[start..end].join(" "));
            if end >= words.len() {
                break;
            }
            start += step;
        }
        chunks
    }

    // ── Recursive strategy ───────────────────────────────────────────────────

    /// Recursive chunking: splits by the first (coarsest) separator, then
    /// recursively applies finer separators to pieces that still exceed
    /// `chunk_size`.
    fn chunk_recursive(text: &str, chunk_size: usize, separators: &[String]) -> Vec<String> {
        // No more separators → fall back to word-level (no overlap).
        if separators.is_empty() || (separators.len() == 1 && separators[0].is_empty()) {
            return Self::chunk_fixed(text, chunk_size, 0);
        }

        let current_sep = &separators[0];
        let remaining = &separators[1..];
        let parts: Vec<&str> = text.split(current_sep).collect();

        let mut result: Vec<String> = Vec::new();
        let mut current: Vec<&str> = Vec::new(); // accumulated pieces for the current chunk

        for part in &parts {
            if part.is_empty() {
                continue;
            }

            let part_words = part.split_whitespace().count();

            // Single piece exceeds budget → recurse with finer separator.
            if part_words > chunk_size {
                // Flush any accumulated pieces first.
                if !current.is_empty() {
                    result.push(current.join(current_sep));
                    current.clear();
                }
                let sub = Self::chunk_recursive(part, chunk_size, remaining);
                result.extend(sub);
                continue;
            }

            // Would adding this piece overflow the chunk?  Flush first.
            let current_words: usize = current
                .iter()
                .map(|p| p.split_whitespace().count())
                .sum();

            if current_words + part_words > chunk_size && !current.is_empty() {
                result.push(current.join(current_sep));
                current.clear();
            }

            current.push(part);
        }

        // Flush the final chunk.
        if !current.is_empty() {
            result.push(current.join(current_sep));
        }

        result
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

    // ── Fixed strategy tests ─────────────────────────────────────────────────

    #[test]
    fn test_text_chunker_fixed_default() {
        let chunker = TextChunker::new(5, 2);
        let text = "This is a test of the chunking system for long documents";
        let chunks = chunker.chunk(text);

        assert!(!chunks.is_empty());
        assert!(chunks[0].contains("This"));
    }

    #[test]
    fn test_chunk_fixed_empty() {
        let chunks = TextChunker::chunk_fixed("", 5, 2);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_fixed_exact_size() {
        let chunks = TextChunker::chunk_fixed("one two three", 3, 1);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "one two three");
    }

    #[test]
    fn test_chunk_fixed_overlap_produces_multiple() {
        let chunks = TextChunker::chunk_fixed("a b c d e f g", 3, 1);
        assert!(chunks.len() > 1);
        assert!(chunks[0].starts_with('a'));
        // Overlap: second chunk should share some words with first
        assert!(chunks[1].contains('c'));
    }

    #[test]
    fn test_chunk_fixed_overlap_equals_size_no_infinite_loop() {
        // When overlap >= size the step is clamped to 1.
        let chunks = TextChunker::chunk_fixed("a b c d e", 3, 3);
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn test_chunk_fixed_overlap_greater_than_size() {
        let chunks = TextChunker::chunk_fixed("a b c d e", 3, 5);
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn test_chunk_fixed_zero_size() {
        let chunks = TextChunker::chunk_fixed("hello world", 0, 0);
        assert!(chunks.is_empty());
    }

    // ── Recursive strategy tests ─────────────────────────────────────────────

    #[test]
    fn test_chunk_recursive_basic() {
        let text = "Paragraph one.\n\nParagraph two.\n\nParagraph three.";
        let chunks = TextChunker::chunk_recursive(text, 50, &default_recursive_separators());
        assert!(!chunks.is_empty());
        // Each paragraph is < 50 words, so they stay separate
        assert!(chunks.iter().any(|c| c.contains("Paragraph one")));
        assert!(chunks.iter().any(|c| c.contains("Paragraph two")));
    }

    #[test]
    fn test_chunk_recursive_long_paragraph() {
        let words: Vec<String> = (0..200).map(|i| format!("word{}", i)).collect();
        let long_para = words.join(" ");
        let text = format!("Short lead.\n\n{}\n\nShort tail.", long_para);

        let chunks = TextChunker::chunk_recursive(&text, 100, &default_recursive_separators());
        // The long paragraph should be split across multiple chunks
        assert!(chunks.len() >= 2, "long paragraph should be split: got {} chunks", chunks.len());
        // Each chunk should contain at most ~100 words
        for chunk in &chunks {
            let wc = chunk.split_whitespace().count();
            assert!(wc <= 110, "chunk has {} words, expected <= 110", wc);
        }
    }

    #[test]
    fn test_chunk_recursive_empty_text() {
        let chunks = TextChunker::chunk_recursive("", 100, &default_recursive_separators());
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunk_recursive_falls_back_to_fixed() {
        let text = "a b c d e f g h i j";
        // Single-character separator forces fallback to word-level
        let chunks = TextChunker::chunk_recursive(text, 3, &[" ".to_string()]);
        assert_eq!(chunks.len(), 4, "10 words / 3 per chunk = 4 chunks");
    }

    #[test]
    fn test_chunk_recursive_small_chunk_size() {
        let text = "Hello world.\nFoo bar baz.\n\nLast para.";
        let chunks = TextChunker::chunk_recursive(text, 2, &default_recursive_separators());
        assert!(chunks.len() >= 2, "should split into at least 2 chunks");
    }

    // ── Strategy dispatch tests ──────────────────────────────────────────────

    #[test]
    fn test_chunker_with_fixed_strategy() {
        let strategy = ChunkStrategy::Fixed { chunk_size: 5, chunk_overlap: 1 };
        let chunker = TextChunker::with_strategy(strategy);
        let text = "a b c d e f g";
        let chunks = chunker.chunk(text);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn test_chunker_with_recursive_strategy() {
        let strategy = ChunkStrategy::Recursive {
            chunk_size: 50,
            separators: None,
        };
        let chunker = TextChunker::with_strategy(strategy);
        let text = "Para A.\n\nPara B.\n\nPara C.";
        let chunks = chunker.chunk(text);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_chunk_strategy_default_is_recursive() {
        let strategy = ChunkStrategy::default();
        assert!(matches!(strategy, ChunkStrategy::Recursive { .. }));
    }

    #[test]
    fn test_chunker_new_uses_fixed() {
        let chunker = TextChunker::new(3, 1);
        assert!(matches!(chunker.strategy(), ChunkStrategy::Fixed { .. }));
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
        assert!(chunker.chunk("").is_empty());
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
