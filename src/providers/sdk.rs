//! Provider SDK Extension Layer
//!
//! Extends the core provider trait with capability discovery,
//! provider packs, and dynamic registration hooks.
//!
//! Design matches OpenClaw's provider extension architecture.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Capabilities advertised by a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Supports streaming completions.
    pub streaming: bool,
    /// Supports vision (image input).
    pub vision: bool,
    /// Supports function calling / tool use.
    pub function_calling: bool,
    /// Supports structured output (JSON schema).
    pub structured_output: bool,
    /// Supports embeddings.
    pub embeddings: bool,
    /// Maximum context length (tokens).
    pub max_context_length: Option<u32>,
    /// Model families supported (e.g. ["gpt-4", "gpt-3.5"]).
    pub model_families: Vec<String>,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            streaming: true,
            vision: false,
            function_calling: true,
            structured_output: false,
            embeddings: false,
            max_context_length: None,
            model_families: vec![],
        }
    }
}

/// Metadata for a registered provider.
#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    pub name: String,
    pub capabilities: ProviderCapabilities,
    pub config_schema: serde_json::Value,
    pub health_status: ProviderHealth,
}

/// Health status of a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealth {
    Healthy,
    Degraded { reason: String },
    Unhealthy { reason: String },
    Unknown,
}

impl Default for ProviderHealth {
    fn default() -> Self {
        ProviderHealth::Unknown
    }
}

/// A "provider pack" — a bundle of related providers shipped as a unit.
///
/// OpenClaw uses provider packs to group providers by vendor or capability.
pub struct ProviderPack {
    pub name: String,
    pub version: String,
    pub providers: Vec<String>,
}

/// Provider SDK — entry point for dynamic provider registration.
pub struct ProviderSdk {
    packs: HashMap<String, ProviderPack>,
}

impl ProviderSdk {
    pub fn new() -> Self {
        Self { packs: HashMap::new() }
    }

    pub fn register_pack(&mut self, pack: ProviderPack) {
        self.packs.insert(pack.name.clone(), pack);
    }

    pub fn list_packs(&self) -> Vec<&ProviderPack> {
        self.packs.values().collect()
    }

    /// Synchronize provider packs from the ModelRouter.
    ///
    /// Creates a provider pack containing all providers currently
    /// registered in the model router.
    pub async fn sync_from_model_router(
        &mut self,
        model_router: &crate::model_router::ModelRouter,
    ) {
        let providers = model_router.list_providers().await;
        if providers.is_empty() {
            return;
        }
        let names: Vec<String> = providers.iter().map(|p| p.name.clone()).collect();
        self.register_pack(ProviderPack {
            name: "model_router_sync".to_string(),
            version: "1.0".to_string(),
            providers: names,
        });
    }
}

impl Default for ProviderSdk {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capabilities_default() {
        let caps = ProviderCapabilities::default();
        assert!(caps.streaming);
        assert!(!caps.vision);
    }

    #[test]
    fn test_provider_pack() {
        let mut sdk = ProviderSdk::new();
        sdk.register_pack(ProviderPack {
            name: "openai_pack".to_string(),
            version: "1.0".to_string(),
            providers: vec!["openai".to_string(), "openai_embedding".to_string()],
        });
        assert_eq!(sdk.list_packs().len(), 1);
    }
}
