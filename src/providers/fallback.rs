//! Fallback provider implementation for Manta
//!
//! This provider wraps multiple providers and tries them in order until one succeeds.

use super::{CompletionRequest, CompletionResponse, CompletionStream, Provider};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// A provider that falls back to other providers on failure
pub struct FallbackProvider {
    /// List of providers to try in order
    providers: Vec<Arc<dyn Provider>>,
    /// Name of this provider
    name: String,
}

impl std::fmt::Debug for FallbackProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackProvider")
            .field("name", &self.name)
            .field("providers", &self.providers.len())
            .finish()
    }
}

impl FallbackProvider {
    /// Create a new fallback provider with the given providers
    pub fn new(name: impl Into<String>, providers: Vec<Arc<dyn Provider>>) -> Self {
        Self { name: name.into(), providers }
    }

    /// Create with default providers (openai -> anthropic)
    pub fn with_defaults(openai: Arc<dyn Provider>, anthropic: Arc<dyn Provider>) -> Self {
        Self::new("fallback", vec![openai, anthropic])
    }

    /// Add a provider to the chain
    pub fn add_provider(&mut self, provider: Arc<dyn Provider>) {
        self.providers.push(provider);
    }

    /// Get the list of provider names in the chain
    pub fn chain(&self) -> Vec<String> {
        self.providers
            .iter()
            .map(|p| p.name().to_string())
            .collect()
    }
}

#[async_trait]
impl Provider for FallbackProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn default_model(&self) -> &str {
        // Return the default model of the first provider
        self.providers
            .first()
            .map(|p| p.default_model())
            .unwrap_or("unknown")
    }

    fn supports_tools(&self) -> bool {
        // All providers in chain must support tools
        self.providers.iter().all(|p| p.supports_tools())
    }

    fn max_context(&self) -> usize {
        // Return the minimum context size across all providers
        self.providers
            .iter()
            .map(|p| p.max_context())
            .min()
            .unwrap_or(4096)
    }

    async fn complete(&self, request: CompletionRequest) -> crate::Result<CompletionResponse> {
        let mut last_error = None;

        for (idx, provider) in self.providers.iter().enumerate() {
            let provider_name = provider.name();
            debug!("Trying provider {}: {}", idx + 1, provider_name);

            match provider.complete(request.clone()).await {
                Ok(response) => {
                    info!("Provider {} succeeded: {}", idx + 1, provider_name);
                    return Ok(response);
                }
                Err(e) => {
                    warn!("Provider {} failed: {} - Error: {}", idx + 1, provider_name, e);
                    last_error = Some(e);
                }
            }
        }

        error!("All providers in fallback chain failed");
        Err(crate::error::MantaError::ExternalService {
            source: "All providers in fallback chain failed".to_string(),
            cause: last_error.map(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
        })
    }

    async fn stream(&self, request: CompletionRequest) -> crate::Result<CompletionStream> {
        let mut last_error = None;

        for (idx, provider) in self.providers.iter().enumerate() {
            let provider_name = provider.name();
            debug!("Trying provider {} for streaming: {}", idx + 1, provider_name);

            match provider.stream(request.clone()).await {
                Ok(stream) => {
                    info!("Provider {} succeeded for streaming: {}", idx + 1, provider_name);
                    return Ok(stream);
                }
                Err(e) => {
                    warn!(
                        "Provider {} failed for streaming: {} - Error: {}",
                        idx + 1,
                        provider_name,
                        e
                    );
                    last_error = Some(e);
                }
            }
        }

        error!("All providers in fallback chain failed for streaming");
        Err(crate::error::MantaError::ExternalService {
            source: "All providers in fallback chain failed for streaming".to_string(),
            cause: last_error.map(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>),
        })
    }

    async fn health_check(&self) -> crate::Result<bool> {
        // Check if any provider is healthy
        for provider in &self.providers {
            match provider.health_check().await {
                Ok(true) => return Ok(true),
                Ok(false) => continue,
                Err(_) => continue,
            }
        }
        Ok(false)
    }
}

/// Builder for creating fallback chains
#[derive(Default)]
pub struct FallbackChainBuilder {
    providers: Vec<Arc<dyn Provider>>,
}

impl std::fmt::Debug for FallbackChainBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackChainBuilder")
            .field("providers", &self.providers.len())
            .finish()
    }
}

impl FallbackChainBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a provider to the chain
    pub fn with_provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// Build the fallback provider
    pub fn build(self, name: impl Into<String>) -> FallbackProvider {
        FallbackProvider::new(name, self.providers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Message;

    struct MockProvider {
        name: String,
        default_model: String,
        supports_tools: bool,
        max_context: usize,
        healthy: bool,
        fail_complete: bool,
        fail_stream: bool,
    }

    impl MockProvider {
        fn new(name: impl Into<String>) -> Self {
            Self {
                name: name.into(),
                default_model: "mock-model".to_string(),
                supports_tools: true,
                max_context: 4096,
                healthy: true,
                fail_complete: false,
                fail_stream: false,
            }
        }

        fn with_max_context(mut self, max: usize) -> Self {
            self.max_context = max;
            self
        }

        fn with_supports_tools(mut self, supports: bool) -> Self {
            self.supports_tools = supports;
            self
        }

        fn failing_complete(mut self) -> Self {
            self.fail_complete = true;
            self
        }

        fn failing_stream(mut self) -> Self {
            self.fail_stream = true;
            self
        }

        fn unhealthy(mut self) -> Self {
            self.healthy = false;
            self
        }
    }

    #[async_trait]
    impl Provider for MockProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn default_model(&self) -> &str {
            &self.default_model
        }
        fn supports_tools(&self) -> bool {
            self.supports_tools
        }
        fn max_context(&self) -> usize {
            self.max_context
        }
        async fn complete(&self, _req: CompletionRequest) -> crate::Result<CompletionResponse> {
            if self.fail_complete {
                Err(crate::error::MantaError::Internal("fail".to_string()))
            } else {
                Ok(CompletionResponse {
                    message: Message::assistant("ok"),
                    usage: None,
                    model: self.default_model.clone(),
                    finish_reason: None,
                })
            }
        }
        async fn stream(&self, _req: CompletionRequest) -> crate::Result<CompletionStream> {
            if self.fail_stream {
                Err(crate::error::MantaError::Internal("fail".to_string()))
            } else {
                Ok(Box::pin(tokio_stream::iter(vec![])))
            }
        }
        async fn health_check(&self) -> crate::Result<bool> {
            Ok(self.healthy)
        }
    }

    #[test]
    fn test_fallback_provider_creation() {
        let fallback = FallbackProvider::new("test", vec![]);
        assert_eq!(fallback.name(), "test");
        assert_eq!(fallback.chain().len(), 0);
    }

    #[test]
    fn test_fallback_builder() {
        let builder = FallbackChainBuilder::new();
        let fallback = builder.build("my-fallback");

        assert_eq!(fallback.name(), "my-fallback");
        assert_eq!(fallback.chain().len(), 0);
    }

    #[test]
    fn test_fallback_default_model_empty() {
        let fallback = FallbackProvider::new("test", vec![]);
        assert_eq!(fallback.default_model(), "unknown");
    }

    #[test]
    fn test_fallback_default_model_first() {
        let p1 = Arc::new(MockProvider::new("p1").with_max_context(8192));
        let fallback = FallbackProvider::new("test", vec![p1]);
        assert_eq!(fallback.default_model(), "mock-model");
    }

    #[test]
    fn test_fallback_max_context_empty() {
        let fallback = FallbackProvider::new("test", vec![]);
        assert_eq!(fallback.max_context(), 4096);
    }

    #[test]
    fn test_fallback_max_context_min() {
        let p1 = Arc::new(MockProvider::new("p1").with_max_context(8192));
        let p2 = Arc::new(MockProvider::new("p2").with_max_context(2048));
        let fallback = FallbackProvider::new("test", vec![p1, p2]);
        assert_eq!(fallback.max_context(), 2048);
    }

    #[test]
    fn test_fallback_supports_tools_all() {
        let p1 = Arc::new(MockProvider::new("p1").with_supports_tools(true));
        let p2 = Arc::new(MockProvider::new("p2").with_supports_tools(true));
        let fallback = FallbackProvider::new("test", vec![p1, p2]);
        assert!(fallback.supports_tools());
    }

    #[test]
    fn test_fallback_supports_tools_one_false() {
        let p1 = Arc::new(MockProvider::new("p1").with_supports_tools(true));
        let p2 = Arc::new(MockProvider::new("p2").with_supports_tools(false));
        let fallback = FallbackProvider::new("test", vec![p1, p2]);
        assert!(!fallback.supports_tools());
    }

    #[test]
    fn test_fallback_chain() {
        let p1 = Arc::new(MockProvider::new("alpha"));
        let p2 = Arc::new(MockProvider::new("beta"));
        let fallback = FallbackProvider::new("test", vec![p1, p2]);
        let chain = fallback.chain();
        assert_eq!(chain, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_fallback_add_provider() {
        let mut fallback = FallbackProvider::new("test", vec![]);
        fallback.add_provider(Arc::new(MockProvider::new("p1")));
        fallback.add_provider(Arc::new(MockProvider::new("p2")));
        assert_eq!(fallback.chain(), vec!["p1", "p2"]);
    }

    #[test]
    fn test_fallback_debug() {
        let fallback = FallbackProvider::new("test", vec![]);
        let debug = format!("{:?}", fallback);
        assert!(debug.contains("FallbackProvider"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn test_fallback_chain_builder_debug() {
        let builder = FallbackChainBuilder::new();
        let debug = format!("{:?}", builder);
        assert!(debug.contains("FallbackChainBuilder"));
    }

    #[tokio::test]
    async fn test_fallback_complete_first_succeeds() {
        let p1 = Arc::new(MockProvider::new("p1"));
        let p2 = Arc::new(MockProvider::new("p2"));
        let fallback = FallbackProvider::new("test", vec![p1, p2]);
        let req = CompletionRequest::default();
        let result = fallback.complete(req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fallback_complete_fallback_to_second() {
        let p1 = Arc::new(MockProvider::new("p1").failing_complete());
        let p2 = Arc::new(MockProvider::new("p2"));
        let fallback = FallbackProvider::new("test", vec![p1, p2]);
        let req = CompletionRequest::default();
        let result = fallback.complete(req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fallback_complete_all_fail() {
        let p1 = Arc::new(MockProvider::new("p1").failing_complete());
        let p2 = Arc::new(MockProvider::new("p2").failing_complete());
        let fallback = FallbackProvider::new("test", vec![p1, p2]);
        let req = CompletionRequest::default();
        let result = fallback.complete(req).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("All providers in fallback chain failed"));
    }

    #[tokio::test]
    async fn test_fallback_stream_first_succeeds() {
        let p1 = Arc::new(MockProvider::new("p1"));
        let fallback = FallbackProvider::new("test", vec![p1]);
        let req = CompletionRequest::default();
        let result = fallback.stream(req).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fallback_stream_all_fail() {
        let p1 = Arc::new(MockProvider::new("p1").failing_stream());
        let fallback = FallbackProvider::new("test", vec![p1]);
        let req = CompletionRequest::default();
        let result = fallback.stream(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fallback_health_check_any_healthy() {
        let p1 = Arc::new(MockProvider::new("p1").unhealthy());
        let p2 = Arc::new(MockProvider::new("p2"));
        let fallback = FallbackProvider::new("test", vec![p1, p2]);
        let result = fallback.health_check().await;
        assert_eq!(result.unwrap(), true);
    }

    #[tokio::test]
    async fn test_fallback_health_check_all_unhealthy() {
        let p1 = Arc::new(MockProvider::new("p1").unhealthy());
        let p2 = Arc::new(MockProvider::new("p2").unhealthy());
        let fallback = FallbackProvider::new("test", vec![p1, p2]);
        let result = fallback.health_check().await;
        assert_eq!(result.unwrap(), false);
    }

    #[tokio::test]
    async fn test_fallback_health_check_empty() {
        let fallback = FallbackProvider::new("test", vec![]);
        let result = fallback.health_check().await;
        assert_eq!(result.unwrap(), false);
    }

    #[test]
    fn test_fallback_chain_builder_add() {
        let p1 = Arc::new(MockProvider::new("p1"));
        let p2 = Arc::new(MockProvider::new("p2"));
        let fallback = FallbackChainBuilder::new()
            .with_provider(p1)
            .with_provider(p2)
            .build("chain");
        assert_eq!(fallback.chain(), vec!["p1", "p2"]);
    }

    #[test]
    fn test_fallback_with_defaults() {
        let openai = Arc::new(MockProvider::new("openai"));
        let anthropic = Arc::new(MockProvider::new("anthropic"));
        let fallback = FallbackProvider::with_defaults(openai, anthropic);
        assert_eq!(fallback.name(), "fallback");
        assert_eq!(fallback.chain(), vec!["openai", "anthropic"]);
    }
}
