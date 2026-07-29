//! Cross-encoder reranking for RAG retrieval.
//!
//! Re-ranks candidate results using a cross-encoder model (via API or local)
//! for more accurate relevance scoring than bi-encoder similarity alone.
//!
//! Provides:
//! - [`Reranker`] trait — abstract over reranking backends
//! - [`NoopReranker`] — pass-through (default)
//! - [`CohereReranker`] — Cohere Rerank API client

use async_trait::async_trait;

use crate::rag::hybrid::HybridSearchResult;

/// Re-rank a set of candidate results by query-candidate relevance.
///
/// Implementations can use API-based cross-encoders, local models, or any
/// other scoring function.  Results are returned in descending relevance
/// order (most relevant first).
#[async_trait]
pub trait Reranker: Send + Sync {
    /// Re-rank `candidates` for the given `query`.
    ///
    /// Returns the re-ranked subset (may truncate to `top_k` or fewer).
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<HybridSearchResult>,
    ) -> crate::Result<Vec<HybridSearchResult>>;
}

/// Pass-through reranker that returns candidates unchanged.
pub struct NoopReranker;

#[async_trait]
impl Reranker for NoopReranker {
    async fn rerank(
        &self,
        _query: &str,
        candidates: Vec<HybridSearchResult>,
    ) -> crate::Result<Vec<HybridSearchResult>> {
        Ok(candidates)
    }
}

/// Cohere Rerank API client.
///
/// Calls the [Cohere Rerank endpoint](https://docs.cohere.com/reference/rerank)
/// to score each candidate document against the query, then sorts by the
/// returned relevance score.
pub struct CohereReranker {
    /// Cohere API key.
    api_key: String,
    /// Model name (e.g. "rerank-english-v3.0").
    model: String,
    /// Maximum number of results to return.
    top_k: usize,
    /// Optional base URL override.
    base_url: String,
    /// HTTP client.
    client: reqwest::Client,
}

impl CohereReranker {
    /// Create a new Cohere reranker.
    ///
    /// Defaults: model = `"rerank-english-v3.0"`, top_k = `10`.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: "rerank-english-v3.0".to_string(),
            top_k: 10,
            base_url: "https://api.cohere.ai/v1/rerank".to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Override the model name.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the number of results to return.
    pub fn with_top_k(mut self, top_k: usize) -> Self {
        self.top_k = top_k;
        self
    }

    /// Override the API base URL.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

#[async_trait]
impl Reranker for CohereReranker {
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<HybridSearchResult>,
    ) -> crate::Result<Vec<HybridSearchResult>> {
        if candidates.is_empty() {
            return Ok(candidates);
        }

        let documents: Vec<String> = candidates.iter().map(|r| r.content.clone()).collect();

        let payload = serde_json::json!({
            "query": query,
            "documents": documents,
            "model": self.model,
            "top_n": self.top_k.min(candidates.len()),
        });

        let response = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: format!("Cohere rerank API request: {}", e),
                cause: None,
            })?;

        let body: serde_json::Value =
            response
                .json()
                .await
                .map_err(|e| crate::error::SyscityError::ExternalService {
                    source: format!("Failed to parse Cohere rerank response: {}", e),
                    cause: None,
                })?;

        let results = body["results"].as_array().ok_or_else(|| {
            crate::error::SyscityError::ExternalService {
                source: "Cohere rerank response missing 'results' array".to_string(),
                cause: None,
            }
        })?;

        let mut reranked: Vec<(usize, f32)> = results
            .iter()
            .filter_map(|r| {
                let idx = r["index"].as_u64()?;
                let score = r["relevance_score"].as_f64()?;
                Some((idx as usize, score as f32))
            })
            .collect();

        // Sort by relevance score descending.
        reranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(reranked
            .into_iter()
            .filter_map(|(idx, score)| {
                let mut result = candidates.get(idx)?.clone();
                result.score = score;
                Some(result)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_reranker() {
        let reranker = NoopReranker;
        let candidates = vec![HybridSearchResult {
            content: "a".to_string(),
            score: 0.9,
            source: "vector".to_string(),
            memory_type: "semantic".to_string(),
            citation: "doc:a".to_string(),
        }];
        let result = reranker.rerank("test", candidates.clone()).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].content, "a");
    }

    #[tokio::test]
    async fn test_noop_reranker_empty() {
        let reranker = NoopReranker;
        let result = reranker.rerank("test", vec![]).await.unwrap();
        assert!(result.is_empty());
    }
}
