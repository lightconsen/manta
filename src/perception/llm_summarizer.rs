//! Bridge from [`crate::providers::Provider`] to
//! [`PerceptionSummarizer`].
//!
//! `perception` deliberately stays decoupled from the full provider /
//! routing stack — it just declares a tiny [`PerceptionSummarizer`]
//! trait. This module wraps an `Arc<dyn Provider>` (and optional model
//! override) so the gateway can hand a real LLM to
//! [`super::MinimalAdapter`] without leaking provider types into the
//! perception module.

use std::sync::Arc;

use async_trait::async_trait;

use crate::perception::{AdapterError, PerceptionSummarizer};
use crate::providers::{CompletionRequest, Message, Provider};

/// Wrap an `Arc<dyn Provider>` so it satisfies [`PerceptionSummarizer`].
pub struct LlmProviderSummarizer {
    provider: Arc<dyn Provider>,
    model: Option<String>,
    max_tokens: u32,
    temperature: f32,
}

impl LlmProviderSummarizer {
    /// Wrap the provider with default parameters
    /// (max_tokens=256, temperature=0.2).
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            model: None,
            max_tokens: 256,
            temperature: 0.2,
        }
    }

    /// Override the model used for summarization (otherwise the
    /// provider's default model is used).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Override the max-tokens cap (default 256).
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Override the temperature (default 0.2 — summaries should be tight).
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }
}

#[async_trait]
impl PerceptionSummarizer for LlmProviderSummarizer {
    async fn summarize(&self, system: &str, user: &str) -> Result<String, AdapterError> {
        let request = CompletionRequest {
            messages: vec![Message::system(system), Message::user(user)],
            tools: None,
            temperature: Some(self.temperature),
            max_tokens: Some(self.max_tokens),
            stream: false,
            model: self.model.clone(),
            stop: None,
            extra: None,
            requires_vision: false,
            requires_tools: false,
            requires_reasoning: false,
            fallback_models: Vec::new(),
        };

        let resp = self
            .provider
            .complete(request)
            .await
            .map_err(|e| AdapterError::Summarizer(e.to_string()))?;

        Ok(resp.message.content)
    }
}
