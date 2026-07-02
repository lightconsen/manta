//! Plugin-extensible Provider
//!
//! Allows WASM plugins to register custom LLM provider implementations.
//! The plugin declares its provider capability in the manifest, and the
//! PluginManager creates a `PluginProvider` that delegates complete/stream
//! calls to the plugin runtime.

use std::sync::Arc;

use async_trait::async_trait;

use crate::providers::stream_wrappers::ProviderStreamFamily;
use crate::providers::{
    CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream, Provider,
};

/// A provider implementation backed by a plugin.
///
/// The plugin must implement the `provider_complete` and `provider_stream`
/// exports (or the runtime must handle the delegation).
pub struct PluginProvider {
    plugin_id: String,
    provider_name: String,
    default_model: String,
    supports_tools: bool,
    max_context: usize,
    stream_family: ProviderStreamFamily,
    runtime: Arc<crate::plugins::runtime::PluginRuntime>,
}

impl PluginProvider {
    /// Create a new plugin-backed provider.
    pub fn new(
        plugin_id: String,
        name: String,
        default_model: String,
        supports_tools: bool,
        max_context: usize,
        stream_family: ProviderStreamFamily,
        runtime: Arc<crate::plugins::runtime::PluginRuntime>,
    ) -> Self {
        Self {
            plugin_id,
            provider_name: name,
            default_model,
            supports_tools,
            max_context,
            stream_family,
            runtime,
        }
    }

    /// Parse a stream family string into the enum.
    pub fn parse_stream_family(family: &str) -> ProviderStreamFamily {
        match family.to_lowercase().as_str() {
            "openai" => ProviderStreamFamily::OpenAi,
            "anthropic" => ProviderStreamFamily::Anthropic,
            "openai_reasoning" | "openai-reasoning" => ProviderStreamFamily::OpenAiReasoning,
            "anthropic_thinking" | "anthropic-thinking" => ProviderStreamFamily::AnthropicThinking,
            "google_thinking" | "google-thinking" => ProviderStreamFamily::GoogleThinking,
            "openrouter" => ProviderStreamFamily::OpenRouter,
            _ => ProviderStreamFamily::Generic,
        }
    }
}

#[async_trait]
impl Provider for PluginProvider {
    fn name(&self) -> &str {
        &self.provider_name
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

    async fn complete(&self, request: CompletionRequest) -> crate::Result<CompletionResponse> {
        // Delegate to plugin runtime
        let result = self
            .runtime
            .call_provider_complete(
                &self.plugin_id,
                &serde_json::to_value(request).unwrap_or_default(),
            )
            .await?;

        let response: CompletionResponse = serde_json::from_value(result).map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "Plugin provider response parse error: {}",
                e
            ))
        })?;
        Ok(response)
    }

    async fn stream(&self, request: CompletionRequest) -> crate::Result<CompletionStream> {
        // For plugin providers, we delegate to the runtime which returns
        // a stream of JSON chunks. We convert those into CompletionChunks.
        let result = self
            .runtime
            .call_provider_stream(
                &self.plugin_id,
                &serde_json::to_value(request).unwrap_or_default(),
            )
            .await?;

        // The plugin runtime returns a receiver or a stream descriptor.
        // For now, we return an empty stream as a placeholder.
        // In a full implementation, the runtime would return a channel receiver
        // that we convert into a CompletionStream.
        let chunks: Vec<CompletionChunk> = serde_json::from_value(result).unwrap_or_default();
        Ok(Box::pin(futures::stream::iter(chunks)))
    }

    fn stream_family(&self) -> ProviderStreamFamily {
        self.stream_family
    }

    async fn health_check(&self) -> crate::Result<bool> {
        match self
            .runtime
            .call_provider_health_check(&self.plugin_id)
            .await
        {
            Ok(val) => Ok(val.as_bool().unwrap_or(false)),
            Err(_) => Ok(false),
        }
    }

    async fn set_credential(
        &self,
        _credential: crate::model_router::Credential,
    ) -> crate::Result<()> {
        // Plugin providers manage credentials inside the plugin runtime; this
        // provider adapter does not hold credentials directly.
        Ok(())
    }
}

/// Registry of plugin-backed providers.
#[derive(Default)]
pub struct PluginProviderRegistry {
    providers: std::collections::HashMap<String, Arc<PluginProvider>>,
}

impl PluginProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            providers: std::collections::HashMap::new(),
        }
    }

    /// Register a plugin provider.
    pub fn register(&mut self, name: String, provider: Arc<PluginProvider>) {
        self.providers.insert(name, provider);
    }

    /// Get a provider by name.
    pub fn get(&self, name: &str) -> Option<Arc<PluginProvider>> {
        self.providers.get(name).cloned()
    }

    /// Remove a provider.
    pub fn remove(&mut self, name: &str) -> Option<Arc<PluginProvider>> {
        self.providers.remove(name)
    }

    /// List all registered plugin provider names.
    pub fn list(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a provider is registered.
    pub fn has(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }
}
