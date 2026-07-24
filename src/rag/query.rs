//! Query transformation for RAG retrieval quality.
//!
//! Provides the [`QueryTransformer`] trait which can rewrite or expand a
//! user query before embedding, improving retrieval quality.  Includes a
//! [`NoopTransformer`] that passes the query through unchanged (default).
//!
//! Domain-specific transformers (e.g. HyDE, query expansion) live in
//! `src/memory/query.rs` so they can depend on LLM providers and the memory
//! domain without pulling those dependencies into the generic RAG layer.

use async_trait::async_trait;

/// Transform a search query before embedding to improve retrieval quality.
///
/// Implementations can rewrite, expand, or decompose the query.  The
/// transformed query is used *only* for embedding — the original query is
/// preserved for LLM consumption.
#[async_trait]
pub trait QueryTransformer: Send + Sync {
    /// Rewrite or expand the query and return the transformed text.
    async fn transform(&self, query: &str) -> crate::Result<String>;
}

/// Pass-through transformer that returns the query unchanged.
pub struct NoopTransformer;

#[async_trait]
impl QueryTransformer for NoopTransformer {
    async fn transform(&self, query: &str) -> crate::Result<String> {
        Ok(query.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_transformer() {
        let t = NoopTransformer;
        let result = t.transform("hello world").await.unwrap();
        assert_eq!(result, "hello world");
    }

    #[tokio::test]
    async fn test_noop_transformer_empty() {
        let t = NoopTransformer;
        let result = t.transform("").await.unwrap();
        assert_eq!(result, "");
    }
}
