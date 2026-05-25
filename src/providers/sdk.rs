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
#[derive(Default)]
pub enum ProviderHealth {
    Healthy,
    Degraded { reason: String },
    Unhealthy { reason: String },
    #[default]
    Unknown,
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

    #[test]
    fn test_provider_capabilities_default() {
        let caps = ProviderCapabilities::default();
        assert!(caps.streaming);
        assert!(!caps.vision);
        assert!(caps.function_calling);
        assert!(!caps.structured_output);
        assert!(!caps.embeddings);
        assert!(caps.max_context_length.is_none());
        assert!(caps.model_families.is_empty());
    }

    #[test]
    fn test_provider_capabilities_serialization() {
        let caps = ProviderCapabilities {
            streaming: false,
            vision: true,
            function_calling: false,
            structured_output: true,
            embeddings: true,
            max_context_length: Some(128000),
            model_families: vec!["gpt-4".to_string()],
        };
        let json = serde_json::to_string(&caps).unwrap();
        assert!(json.contains("streaming"));
        assert!(json.contains("vision"));
    }

    #[test]
    fn test_provider_health_variants() {
        assert!(matches!(ProviderHealth::default(), ProviderHealth::Unknown));
        let h = ProviderHealth::Degraded { reason: "slow".to_string() };
        let json = serde_json::to_string(&h).unwrap();
        assert!(json.contains("degraded"));
    }

    #[test]
    fn test_provider_sdk_default() {
        let sdk: ProviderSdk = Default::default();
        assert!(sdk.list_packs().is_empty());
    }

    #[test]
    fn test_provider_sdk_register_multiple() {
        let mut sdk = ProviderSdk::new();
        sdk.register_pack(ProviderPack {
            name: "pack-a".to_string(),
            version: "1.0".to_string(),
            providers: vec!["p1".to_string()],
        });
        sdk.register_pack(ProviderPack {
            name: "pack-b".to_string(),
            version: "2.0".to_string(),
            providers: vec!["p2".to_string(), "p3".to_string()],
        });
        assert_eq!(sdk.list_packs().len(), 2);
    }

    #[test]
    fn test_provider_pack_fields() {
        let pack = ProviderPack {
            name: "test".to_string(),
            version: "0.5".to_string(),
            providers: vec!["a".to_string()],
        };
        assert_eq!(pack.name, "test");
        assert_eq!(pack.version, "0.5");
        assert_eq!(pack.providers.len(), 1);
    }

    #[test]
    fn test_provider_metadata_creation() {
        let meta = ProviderMetadata {
            name: "openai".to_string(),
            capabilities: ProviderCapabilities::default(),
            config_schema: serde_json::json!({"type": "object"}),
            health_status: ProviderHealth::Healthy,
        };
        assert_eq!(meta.name, "openai");
        assert!(matches!(meta.health_status, ProviderHealth::Healthy));
    }
}
