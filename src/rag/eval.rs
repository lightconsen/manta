//! Retrieval quality evaluation: recall@k, MRR@k, hit_rate@k.
//!
//! Provides a lightweight harness that runs a set of labelled queries through
//! a [`VectorStore`] and computes standard information-retrieval metrics
//! against known relevant document IDs.
//!
//! # Example
//!
//! ```rust,ignore
//! use syscity::rag::eval::{RetrievalSample, evaluate_retrieval};
//!
//! let samples = vec![RetrievalSample {
//!     query: "Rust ownership".into(),
//!     relevant_doc_ids: vec!["doc-1".into(), "doc-3".into()],
//!     collection: None,
//! }];
//!
//! let metrics = evaluate_retrieval(
//!     &samples,
//!     &*vector_store,
//!     &*embedding_provider,
//!     &[1, 3, 5, 10],
//! ).await?;
//! println!("Recall@5: {:.3}", metrics.recall_at_ks(&[5])[0].1);
//! ```

use serde::{Deserialize, Serialize};

use super::embedding::EmbeddingProvider;
use super::vector_store::VectorStore;

/// A single labelled retrieval sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalSample {
    /// The search query.
    pub query: String,
    /// Known relevant document IDs (matches against chunk `source_id`).
    pub relevant_doc_ids: Vec<String>,
    /// Optional collection filter.
    #[serde(default)]
    pub collection: Option<String>,
}

/// Aggregated retrieval metrics over a set of samples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalMetrics {
    /// Number of samples evaluated.
    pub sample_count: usize,
    /// Recall@k for each requested k: (k, value).
    pub recall_at_k: Vec<(usize, f64)>,
    /// Mean Reciprocal Rank@k: (k, value).
    pub mrr_at_k: Vec<(usize, f64)>,
    /// Hit rate@k: proportion of samples with ≥1 relevant doc in top k.
    pub hit_rate_at_k: Vec<(usize, f64)>,
}

impl RetrievalMetrics {
    /// Returns the recall@k values for the requested ks (convenience helper).
    pub fn recall_at_ks(&self, ks: &[usize]) -> Vec<(usize, f64)> {
        let map: std::collections::HashMap<usize, f64> = self.recall_at_k.iter().cloned().collect();
        ks.iter().filter_map(|k| map.get(k).map(|v| (*k, *v))).collect()
    }

    /// Returns the MRR@k values for the requested ks.
    pub fn mrr_at_ks(&self, ks: &[usize]) -> Vec<(usize, f64)> {
        let map: std::collections::HashMap<usize, f64> = self.mrr_at_k.iter().cloned().collect();
        ks.iter().filter_map(|k| map.get(k).map(|v| (*k, *v))).collect()
    }
}

/// Run retrieval evaluation over labelled samples.
///
/// For each sample:
/// 1. Embeds the query and searches the vector store (requesting
///    `k_values.max()` results).
/// 2. Compares returned chunk `source_id`s against `relevant_doc_ids`.
/// 3. Aggregates recall@k, MRR@k, and hit_rate@k across all samples.
pub async fn evaluate_retrieval(
    samples: &[RetrievalSample],
    vector_store: &dyn VectorStore,
    embedding_provider: &dyn EmbeddingProvider,
    k_values: &[usize],
) -> crate::Result<RetrievalMetrics> {
    if samples.is_empty() || k_values.is_empty() {
        return Ok(RetrievalMetrics {
            sample_count: 0,
            recall_at_k: k_values.iter().map(|&k| (k, 0.0)).collect(),
            mrr_at_k: k_values.iter().map(|&k| (k, 0.0)).collect(),
            hit_rate_at_k: k_values.iter().map(|&k| (k, 0.0)).collect(),
        });
    }

    let max_k = *k_values.iter().max().unwrap_or(&10);

    // Per-k accumulators.
    let sample_count = samples.len() as f64;
    let mut recall_sum: Vec<f64> = vec![0.0; k_values.len()];
    let mut mrr_sum: Vec<f64> = vec![0.0; k_values.len()];
    let mut hit_sum: Vec<f64> = vec![0.0; k_values.len()];

    for sample in samples {
        let query_embedding = embedding_provider.embed(&sample.query).await?;
        // Always request max_k results from the store.
        let results = vector_store
            .search_similar(
                &query_embedding,
                max_k,
                0.0, // no threshold — filter at metric level
                sample.collection.as_deref(),
            )
            .await?;

        // Build set of relevant doc IDs.
        let relevant: std::collections::HashSet<&str> =
            sample.relevant_doc_ids.iter().map(|s| s.as_str()).collect();
        let num_relevant = sample.relevant_doc_ids.len();

        for (ki, &k) in k_values.iter().enumerate() {
            let top_k = results.iter().take(k);

            // Count relevant docs in top_k.
            let found: Vec<&str> = top_k
                .filter_map(|(chunk, _)| {
                    if relevant.contains(chunk.source_id.as_str()) {
                        Some(chunk.source_id.as_str())
                    } else {
                        None
                    }
                })
                .collect();

            let found_count = found.len();
            let hit = if found_count > 0 { 1.0 } else { 0.0 };

            // reciprocal rank: 1 / (first relevant position)
            let rr = results
                .iter()
                .take(k)
                .position(|(chunk, _)| relevant.contains(chunk.source_id.as_str()))
                .map(|pos| 1.0 / (pos as f64 + 1.0))
                .unwrap_or(0.0);

            hit_sum[ki] += hit;
            mrr_sum[ki] += rr;

            if num_relevant > 0 {
                recall_sum[ki] += found_count as f64 / num_relevant as f64;
            }
        }
    }

    let to_ratio = |v: f64| v / sample_count;
    let to_mean = |v: f64| v / sample_count;

    let recall_at_k: Vec<(usize, f64)> = k_values
        .iter()
        .zip(recall_sum)
        .map(|(&k, v)| (k, to_mean(v)))
        .collect();
    let mrr_at_k: Vec<(usize, f64)> = k_values
        .iter()
        .zip(mrr_sum)
        .map(|(&k, v)| (k, to_ratio(v)))
        .collect();
    let hit_rate_at_k: Vec<(usize, f64)> = k_values
        .iter()
        .zip(hit_sum)
        .map(|(&k, v)| (k, to_ratio(v)))
        .collect();

    Ok(RetrievalMetrics {
        sample_count: samples.len(),
        recall_at_k,
        mrr_at_k,
        hit_rate_at_k,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rag::chunk::EmbeddedChunk;
    use crate::rag::vector_store::{MemoryVectorStore, VectorStore};
    use crate::rag::EmbeddingProvider;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Embedding provider that uses a hash of each text to produce two
    /// independent dimensions, giving unique cosine-similarity ratios.
    struct HashProvider;

    fn hash_embed(text: &str) -> Vec<f32> {
        let h: u32 = text
            .bytes()
            .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
        vec![(h & 0xFF) as f32, ((h >> 8) & 0xFF) as f32]
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for HashProvider {
        fn model_name(&self) -> &str {
            "hash"
        }
        fn dimension(&self) -> usize {
            2
        }
        async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|t| hash_embed(t)).collect())
        }
    }

    async fn store_with_chunks(chunks: Vec<EmbeddedChunk>) -> MemoryVectorStore {
        let store = MemoryVectorStore::new(2);
        for c in chunks {
            store.store_chunk(c).await.unwrap();
        }
        store
    }

    fn make_chunk(id: &str, source_id: &str, text: &str) -> EmbeddedChunk {
        EmbeddedChunk {
            id: id.to_string(),
            source_id: source_id.to_string(),
            text: text.to_string(),
            embedding: hash_embed(text),
            position: 0,
            total_chunks: 1,
            collection: None,
            metadata: None,
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_evaluate_retrieval_perfect() {
        let store = store_with_chunks(vec![
            make_chunk("c1", "doc-a", "alpha"),
            make_chunk("c2", "doc-b", "beta"),
            make_chunk("c3", "doc-c", "gamma"),
        ])
        .await;
        let provider = HashProvider;

        let samples = vec![RetrievalSample {
            query: "alpha".to_string(),
            relevant_doc_ids: vec!["doc-a".to_string()],
            collection: None,
        }];

        let metrics = evaluate_retrieval(&samples, &store, &provider, &[1, 3])
            .await
            .unwrap();

        assert_eq!(metrics.sample_count, 1);
        // recall@1: the only relevant doc is in position 0
        assert!((metrics.recall_at_ks(&[1])[0].1 - 1.0).abs() < 1e-6);
        // mrr@1: first relevant is at position 0 → 1/1 = 1.0
        assert!((metrics.mrr_at_ks(&[1])[0].1 - 1.0).abs() < 1e-6);
        // hit_rate@1: hit
        assert!((metrics.hit_rate_at_k[0].1 - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_evaluate_retrieval_no_match() {
        let store = store_with_chunks(vec![
            make_chunk("c1", "doc-a", "alpha"),
            make_chunk("c2", "doc-b", "beta"),
        ])
        .await;
        let provider = HashProvider;

        let samples = vec![RetrievalSample {
            query: "gamma".to_string(),
            relevant_doc_ids: vec!["doc-x".to_string()],
            collection: None,
        }];

        let metrics = evaluate_retrieval(&samples, &store, &provider, &[1, 3])
            .await
            .unwrap();

        assert!((metrics.recall_at_ks(&[3])[0].1).abs() < 1e-6);
        assert!((metrics.mrr_at_ks(&[3])[0].1).abs() < 1e-6);
        assert!((metrics.hit_rate_at_k[1].1).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_evaluate_retrieval_empty_samples() {
        let store = MemoryVectorStore::new(2);
        let provider = HashProvider;

        let metrics = evaluate_retrieval(&[], &store, &provider, &[1, 3])
            .await
            .unwrap();
        assert_eq!(metrics.sample_count, 0);
        assert_eq!(metrics.recall_at_k.len(), 2);
    }

    #[tokio::test]
    async fn test_evaluate_retrieval_mrr_position() {
        // With HashProvider, querying "xxx" produces [120,120] embedding,
        // which has cos=1.0 with the "xxx" chunk (doc-a, position 0).
        let store = store_with_chunks(vec![
            make_chunk("c1", "doc-a", "xxx"),
            make_chunk("c2", "doc-b", "yyy"),
            make_chunk("c3", "doc-c", "zzz"),
        ])
        .await;

        let samples = vec![RetrievalSample {
            query: "xxx".to_string(),
            relevant_doc_ids: vec!["doc-a".to_string()],
            collection: None,
        }];

        let metrics = evaluate_retrieval(&samples, &store, &HashProvider, &[3])
            .await
            .unwrap();

        // doc-a has cos=1.0 → first relevant at position 0 → MRR = 1/1 = 1.0
        assert!((metrics.mrr_at_ks(&[3])[0].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_retrieval_metrics_helpers() {
        let metrics = RetrievalMetrics {
            sample_count: 5,
            recall_at_k: vec![(1, 0.5), (3, 0.7)],
            mrr_at_k: vec![(1, 0.8), (3, 0.9)],
            hit_rate_at_k: vec![(1, 0.6), (3, 0.8)],
        };
        let r = metrics.recall_at_ks(&[1, 3]);
        assert!((r[0].1 - 0.5).abs() < 1e-6);
        assert!((r[1].1 - 0.7).abs() < 1e-6);
        let m = metrics.mrr_at_ks(&[1]);
        assert!((m[0].1 - 0.8).abs() < 1e-6);
    }
}
