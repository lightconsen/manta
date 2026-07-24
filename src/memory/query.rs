//! Memory-domain query transformers.
//!
//! Provides domain-specific [`QueryTransformer`] implementations that depend
//! on LLM providers or memory constructs:
//!
//! - [`HydeTransformer`] — Hypothetical Document Embeddings (HyDE): generates
//!   a hypothetical answer to the query, then uses that as the embedding text.

use std::sync::Arc;

use async_trait::async_trait;

use crate::providers::{CompletionRequest, Message, Provider};
use crate::rag::query::QueryTransformer;

/// HyDE (Hypothetical Document Embeddings) transformer.
///
/// Generates a hypothetical answer to the user's query using an LLM, then
/// uses that answer as the text to embed.  This can improve retrieval when
/// the query is short or ambiguous, because the hypothetical document
/// surface is closer to the target documents in embedding space.
pub struct HydeTransformer {
    /// LLM provider used to generate the hypothetical document.
    provider: Arc<dyn Provider>,
    /// Optional model override (uses provider default when `None`).
    model: Option<String>,
}

impl HydeTransformer {
    /// Create a new HyDE transformer backed by `provider`.
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            model: None,
        }
    }

    /// Override the model used for generation.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

#[async_trait]
impl QueryTransformer for HydeTransformer {
    async fn transform(&self, query: &str) -> crate::Result<String> {
        let prompt = format!(
            "You are a helpful assistant. Given a user's query, write a short, \
             factual paragraph that answers it. Do not include any meta-commentary \
             or prefacing — just write the answer.\n\nQuery: {query}"
        );

        let request = CompletionRequest {
            messages: vec![
                Message::system(
                    "You are a helpful assistant. Write a direct, factual answer.",
                ),
                Message::user(prompt),
            ],
            model: self.model.clone(),
            temperature: Some(0.3),
            max_tokens: Some(512),
            stream: false,
            ..Default::default()
        };

        let response = self.provider.complete(request).await?;
        Ok(response.message.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::mock::MockProvider;

    #[tokio::test]
    async fn test_hyde_transformer_with_mock() {
        let mock = Arc::new(
            MockProvider::new().with_callback(|_| {
                Message::assistant(
                    "HyDE: The user asked about Rust's ownership system, \
                     which involves borrowing, lifetimes, and the borrow checker.",
                )
            }),
        );
        let transformer = HydeTransformer::new(mock);
        let result = transformer
            .transform("how does Rust ownership work?")
            .await
            .unwrap();
        assert!(result.contains("ownership"));
    }
}
