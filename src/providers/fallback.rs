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

impl Default for FallbackChainBuilder {
    fn default() -> Self {
        Self { providers: Vec::new() }
    }
}

impl FallbackChainBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a provider to the chain
    pub fn add(mut self, provider: Arc<dyn Provider>) -> Self {
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
}
