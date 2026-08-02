//! ModelRouter — multi-provider LLM routing with fallback chains
//!
//! Provides the primary [`ModelRouter`] struct with support for:
//! - Model aliases (e.g. "fast" → "claude-3-haiku")
//! - Multi-provider routing with automatic fallback
//! - Health checking and circuit breaker
//! - Auth profile / API key rotation
//! - Cost-aware routing with task classification
//! - Pluggable task classifier
//!
//! The `impl ModelRouter` blocks are split across focused submodules:
//! - `init`: construction, builder wiring, initialization
//! - `health`: health checks, circuit breaker, capability routing
//! - `failure`: provider creation, failure recording, key rotation
//! - `routing`: completion/streaming request flow with fallback
//! - `cost_aware`: cost-aware automatic model selection
//! - `quota`: usage snapshots with remote/local quota
//! - `admin`: provider/alias/fallback-chain management

mod admin;
mod cost_aware;
mod failure;
mod health;
mod init;
mod quota;
mod routing;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::future::join_all;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::gateway::task_registry::TaskRegistry;
use crate::model_router::auth_profile::{AuthProfileManager, ProfileStatus};
use crate::model_router::classifier::{KeywordTaskClassifier, TaskClassifierImpl};
use crate::model_router::config::{
    CircuitState, CostAwareConfig, FallbackEntry, ModelAlias, ModelRouterConfig, ProviderConfig,
    ProviderHealth, ProviderHealthInfo, ProviderInfo, ProviderType, TaskType,
};
use crate::model_router::failure_class::FailureClass;
use crate::model_router::model_catalog::{self, ModelCatalog, ModelCatalogEntry};
use crate::model_router::oauth_credential::Credential;
use crate::model_router::usage_fetcher::{
    LocalBudgetFetcher, OpenAiUsageFetcher, UsageFetcher, UsageFetcherRegistry,
};
use crate::model_router::usage_tracker::{
    ProviderUsageSnapshot, ProviderUsageTracker, UsageQuota, UsageTrackerConfig,
};
use crate::providers::{
    CompletionRequest, CompletionResponse, CompletionStream, Message, Provider, ToolDefinition,
    Usage,
};

// ------------------------------------------------------------------
// ModelRouter
// ------------------------------------------------------------------

/// Model router for multi-provider LLM routing.
///
/// # Lock ordering
///
/// To avoid deadlocks, acquire locks in this order and release them before
/// any `.await` that does not strictly need the lock:
///
/// 1. `config`
/// 2. `providers`
/// 3. `health`
/// 4. `usage_fetchers`
///
/// Write locks must never be held across an `.await`.
pub struct ModelRouter {
    /// Configuration
    config: RwLock<ModelRouterConfig>,
    /// Provider instances
    providers: RwLock<HashMap<String, Arc<dyn Provider + Send + Sync>>>,
    /// Health tracking per provider
    health: RwLock<HashMap<String, ProviderHealth>>,
    /// Active fallback chains
    fallback_chains: RwLock<HashMap<String, Vec<FallbackEntry>>>,
    /// Auth profile manager for API key rotation
    pub auth_profiles: AuthProfileManager,
    /// Per-provider usage tracker
    pub usage_tracker: ProviderUsageTracker,
    /// Dynamic model catalog with discovery and suppression
    pub model_catalog: ModelCatalog,
    /// Remote usage quota fetchers keyed by provider name.
    pub usage_fetchers: RwLock<UsageFetcherRegistry>,
    /// Optional SQLite pool for persisting auth profile state across restarts.
    db_pool: Option<sqlx::Pool<sqlx::Sqlite>>,
    /// Optional task registry for tracking the health-check background task.
    task_registry: Option<Arc<TaskRegistry>>,
    /// Shutdown token for cancelling the health-check background task.
    shutdown_token: CancellationToken,
    /// Pluggable task classifier for cost-aware routing.
    classifier: Box<dyn TaskClassifierImpl>,
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::new(ModelRouterConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::gateway::task_registry::TaskRegistry;
    use crate::model_router::auth_profile::{AuthKeyConfig, AuthProfileConfig};
    use crate::model_router::config::{
        CircuitState, CostAwareConfig, ModelAlias, ModelRouterConfig, ProviderConfig, ProviderType,
    };
    use crate::model_router::usage_fetcher::UsageFetcher;
    use crate::model_router::usage_tracker::{QuotaSource, UsageQuota};
    use crate::providers::{
        CompletionRequest, CompletionResponse, CompletionStream, Message, Provider, Usage,
    };

    #[tokio::test]
    async fn default_config_has_no_aliases() {
        let config = ModelRouterConfig::default();
        assert!(config.aliases.is_empty());
        assert_eq!(config.default_model, "");
    }

    #[tokio::test]
    async fn aliases_with_configs_returns_all_aliases() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "fast".to_string(),
                provider: "openai".to_string(),
                model: "gpt-3.5".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_alias(ModelAlias {
                name: "smart".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let aliases = router.aliases_with_configs().await;
        assert_eq!(aliases.len(), 2);
        assert!(aliases.iter().any(|(n, _)| n == "fast"));
        assert!(aliases.iter().any(|(n, _)| n == "smart"));
    }

    #[tokio::test]
    async fn alias_config_returns_single_alias() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let alias = router.alias_config("default").await;
        assert!(alias.is_some());
        assert_eq!(alias.unwrap().provider, "anthropic");
        assert!(router.alias_config("missing").await.is_none());
    }

    #[tokio::test]
    async fn router_config_returns_clone() {
        let mut config = ModelRouterConfig::default();
        config.default_model = "default".to_string();
        let router = ModelRouter::new(config);

        let snapshot = router.router_config().await;
        assert_eq!(snapshot.default_model, "default");
    }

    #[tokio::test]
    async fn list_aliases_returns_empty_by_default() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let aliases = router.list_aliases().await;
        assert!(aliases.is_empty());
    }

    #[tokio::test]
    async fn set_alias_adds_new_alias() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let alias = ModelAlias {
            name: "coding".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            temperature: Some(0.2),
            max_tokens: Some(4096),
        };
        router.set_alias(alias.clone()).await;

        let aliases = router.list_aliases().await;
        assert_eq!(aliases.len(), 1);
        assert!(aliases.contains(&"coding".to_string()));
    }

    #[tokio::test]
    async fn remove_alias_deletes_alias() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "fast".to_string(),
                provider: "openai".to_string(),
                model: "gpt-3.5".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let removed = router.remove_alias("fast").await;
        assert!(removed);

        let aliases = router.list_aliases().await;
        assert!(aliases.is_empty());
    }

    #[tokio::test]
    async fn remove_unknown_alias_returns_false() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let removed = router.remove_alias("nonexistent").await;
        assert!(!removed);
    }

    #[tokio::test]
    async fn switch_default_model_changes_default() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_alias(ModelAlias {
                name: "fast".to_string(),
                provider: "openai".to_string(),
                model: "gpt-3.5".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router.switch_default_model("fast").await.unwrap();

        let default = router.get_default_model().await;
        assert_eq!(default, "fast");
    }

    #[tokio::test]
    async fn switch_unknown_model_fails() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let result = router.switch_default_model("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn list_providers_returns_empty_when_none() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let providers = router.list_providers().await;
        assert_eq!(providers.len(), 0);
    }

    #[tokio::test]
    async fn add_and_remove_provider() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let config = ProviderConfig {
            provider_type: ProviderType::Anthropic,
            api_key: "test-key".to_string().into(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        router.add_provider("test-provider", config).await.unwrap();
        let providers = router.list_providers().await;
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "test-provider");

        router.remove_provider("test-provider").await.unwrap();
        let providers = router.list_providers().await;
        assert_eq!(providers.len(), 0);
    }

    #[tokio::test]
    async fn enable_disable_provider() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let config = ProviderConfig {
            provider_type: ProviderType::Anthropic,
            api_key: "test-key".to_string().into(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        router.add_provider("p1", config).await.unwrap();

        let providers = router.list_providers().await;
        assert!(providers[0].enabled);
        assert_eq!(providers[0].circuit_state, CircuitState::Closed);

        router.disable_provider("p1").await.unwrap();
        let providers = router.list_providers().await;
        assert!(!providers[0].enabled);
        assert_eq!(providers[0].circuit_state, CircuitState::Open);

        router.enable_provider("p1").await.unwrap();
        let providers = router.list_providers().await;
        assert!(providers[0].enabled);
        assert_eq!(providers[0].circuit_state, CircuitState::Closed);
    }

    #[tokio::test]
    async fn enable_unknown_provider_fails() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let result = router.enable_provider("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fallback_chain_roundtrip() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_fallback_chain("default", vec!["p1".to_string(), "p2".to_string()])
            .await
            .unwrap();

        let chain = router.get_fallback_chain("default").await;
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0], "p1");
        assert_eq!(chain[1], "p2");
    }

    #[tokio::test]
    async fn fallback_chain_for_unknown_alias_returns_empty() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let chain = router.get_fallback_chain("nonexistent").await;
        assert!(chain.is_empty());
    }

    #[tokio::test]
    async fn get_provider_health_returns_info() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let config = ProviderConfig {
            provider_type: ProviderType::Anthropic,
            api_key: "test-key".to_string().into(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        router.add_provider("p1", config).await.unwrap();

        let health = router.get_provider_health("p1").await;
        assert!(health.is_some());
        let info = health.unwrap();
        assert_eq!(info.state, "Closed");
        assert_eq!(info.failures, 0);
        assert_eq!(info.successes, 0);
        assert_eq!(info.avg_latency_ms, 0);
    }

    #[tokio::test]
    async fn get_provider_health_unknown_returns_none() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let health = router.get_provider_health("nonexistent").await;
        assert!(health.is_none());
    }

    #[tokio::test]
    async fn check_provider_health_unknown_fails() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let result = router.check_provider_health("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn provider_list_includes_circuit_state() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let config = ProviderConfig {
            provider_type: ProviderType::OpenAi,
            api_key: "key".to_string().into(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };

        router.add_provider("p1", config).await.unwrap();
        let providers = router.list_providers().await;
        assert_eq!(providers[0].circuit_state, CircuitState::Closed);

        router.disable_provider("p1").await.unwrap();
        let providers = router.list_providers().await;
        assert_eq!(providers[0].circuit_state, CircuitState::Open);
    }

    #[tokio::test]
    async fn fallback_chain_set_and_clear() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        router
            .set_fallback_chain("default", vec!["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        let chain = router.get_fallback_chain("default").await;
        assert_eq!(chain, vec!["a", "b"]);

        router.set_fallback_chain("default", vec![]).await.unwrap();
        let chain = router.get_fallback_chain("default").await;
        assert!(chain.is_empty());
    }

    #[tokio::test]
    async fn set_fallback_chain_unknown_alias_fails() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let result = router
            .set_fallback_chain("nonexistent", vec!["a".to_string()])
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn switch_default_model_persists() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router
            .set_alias(ModelAlias {
                name: "default".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_alias(ModelAlias {
                name: "fast".to_string(),
                provider: "openai".to_string(),
                model: "gpt-3.5".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;
        router
            .set_alias(ModelAlias {
                name: "smart".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3-opus".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        router.switch_default_model("fast").await.unwrap();
        assert_eq!(router.get_default_model().await, "fast");

        router.switch_default_model("smart").await.unwrap();
        assert_eq!(router.get_default_model().await, "smart");

        router.switch_default_model("default").await.unwrap();
        assert_eq!(router.get_default_model().await, "default");
    }

    #[tokio::test]
    async fn default_model_is_empty_by_default() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let default = router.get_default_model().await;
        assert_eq!(default, "");
    }

    #[tokio::test]
    async fn cost_aware_disabled_by_default() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        let spend = router.get_daily_spend().await;
        assert_eq!(spend, 0.0);
    }

    #[tokio::test]
    async fn cost_aware_config_default_routing_rules() {
        let config = CostAwareConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.model_costs.len(), 3);
        assert_eq!(config.routing_rules.len(), 9);
        assert_eq!(config.default_alias, "default");
    }

    #[tokio::test]
    async fn reset_daily_spend_no_op_when_disabled() {
        let router = ModelRouter::new(ModelRouterConfig::default());
        router.reset_daily_spend().await;
        assert_eq!(router.get_daily_spend().await, 0.0);
    }

    #[tokio::test]
    async fn cost_aware_config_with_enabled_spend_tracking() {
        let mut config = ModelRouterConfig::default();
        let mut cost_aware = CostAwareConfig::default();
        cost_aware.enabled = true;
        cost_aware.daily_spend_usd = 5.0;
        config.cost_aware = Some(cost_aware);

        let router = ModelRouter::new(config);
        assert_eq!(router.get_daily_spend().await, 5.0);

        router.reset_daily_spend().await;
        assert_eq!(router.get_daily_spend().await, 0.0);
    }

    #[tokio::test]
    async fn health_check_uses_health_check_not_complete() {
        #[derive(Clone)]
        struct TrackingProvider {
            complete_count: Arc<Mutex<usize>>,
            health_check_count: Arc<Mutex<usize>>,
        }

        impl TrackingProvider {
            fn new() -> Self {
                Self {
                    complete_count: Arc::new(Mutex::new(0)),
                    health_check_count: Arc::new(Mutex::new(0)),
                }
            }
        }

        #[async_trait]
        impl Provider for TrackingProvider {
            fn name(&self) -> &str {
                "tracking"
            }
            fn default_model(&self) -> &str {
                "tracking-model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                *self.complete_count.lock().unwrap() += 1;
                Ok(CompletionResponse {
                    message: Message::assistant("ok"),
                    model: "tracking-model".to_string(),
                    usage: Some(Usage::default()),
                    finish_reason: Some("stop".to_string()),
                })
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                *self.health_check_count.lock().unwrap() += 1;
                Ok(true)
            }
            async fn set_credential(
                &self,
                _credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        let mut config = ModelRouterConfig::default();
        config.health_check_interval_secs = 1;
        let registry = Arc::new(TaskRegistry::new());
        let token = CancellationToken::new();
        let router = Arc::new(
            ModelRouter::new(config)
                .with_task_registry(registry.clone())
                .with_shutdown_token(token.clone()),
        );

        let provider = TrackingProvider::new();
        router
            .add_provider_instance("tracking", Arc::new(provider.clone()))
            .await
            .unwrap();

        router.clone().start_health_checks();
        tokio::time::sleep(Duration::from_millis(2500)).await;
        token.cancel();

        assert!(*provider.health_check_count.lock().unwrap() > 0);
        assert_eq!(*provider.complete_count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn health_check_task_respects_shutdown() {
        #[derive(Clone)]
        struct HealthyProvider;

        #[async_trait]
        impl Provider for HealthyProvider {
            fn name(&self) -> &str {
                "healthy"
            }
            fn default_model(&self) -> &str {
                "healthy-model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                Ok(CompletionResponse {
                    message: Message::assistant("ok"),
                    model: "healthy-model".to_string(),
                    usage: Some(Usage::default()),
                    finish_reason: Some("stop".to_string()),
                })
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                Ok(true)
            }
            async fn set_credential(
                &self,
                _credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        let mut config = ModelRouterConfig::default();
        config.health_check_interval_secs = 60;
        let registry = Arc::new(TaskRegistry::new());
        let token = CancellationToken::new();
        let router = Arc::new(
            ModelRouter::new(config)
                .with_task_registry(registry.clone())
                .with_shutdown_token(token.clone()),
        );

        router
            .add_provider_instance("healthy", Arc::new(HealthyProvider))
            .await
            .unwrap();

        router.clone().start_health_checks();

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(registry.contains("model_router:health_check").await);

        token.cancel();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let handle = registry
            .remove_join_or_abort("model_router:health_check")
            .await;
        assert!(handle.is_some());
        let result = tokio::time::timeout(Duration::from_secs(2), handle.unwrap()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn auth_failure_rotates_key_and_retries_successfully() {
        #[derive(Clone)]
        struct RotatingProvider {
            calls: Arc<Mutex<usize>>,
            rotated_key: Arc<Mutex<Option<String>>>,
        }

        impl RotatingProvider {
            fn new() -> Self {
                Self {
                    calls: Arc::new(Mutex::new(0)),
                    rotated_key: Arc::new(Mutex::new(None)),
                }
            }
        }

        #[async_trait]
        impl Provider for RotatingProvider {
            fn name(&self) -> &str {
                "rotator"
            }
            fn default_model(&self) -> &str {
                "rotator-model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                let mut calls = self.calls.lock().unwrap();
                *calls += 1;
                if *calls == 1 {
                    Err(crate::error::SyscityError::ExternalService {
                        source: "OpenAI API error 401: Unauthorized".into(),
                        cause: None,
                    })
                } else {
                    Ok(CompletionResponse {
                        message: Message::assistant("ok"),
                        model: "rotator-model".to_string(),
                        usage: Some(Usage::default()),
                        finish_reason: Some("stop".to_string()),
                    })
                }
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                Ok(true)
            }
            async fn set_credential(
                &self,
                credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                if let crate::model_router::Credential::ApiKey { key } = credential {
                    *self.rotated_key.lock().unwrap() = Some(key);
                }
                Ok(())
            }
        }

        let router = ModelRouter::new(ModelRouterConfig::default());
        let provider = Arc::new(RotatingProvider::new());
        router
            .add_provider_instance("rotator", provider.clone())
            .await
            .unwrap();

        let auth_config = AuthProfileConfig {
            keys: vec![
                AuthKeyConfig {
                    key: "first-key".to_string(),
                    label: "first".to_string(),
                },
                AuthKeyConfig {
                    key: "second-key".to_string(),
                    label: "second".to_string(),
                },
            ],
            cooldown_secs: 60,
            max_failures: 3,
        };
        router
            .auth_profiles
            .register_from_config("rotator", &auth_config)
            .await;

        router
            .set_alias(ModelAlias {
                name: "test".to_string(),
                provider: "rotator".to_string(),
                model: "rotator-model".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let response = router
            .complete("test", vec![Message::user("hi")], None)
            .await
            .expect("complete should succeed after key rotation");
        assert_eq!(response.message.content, "ok");
        assert_eq!(*provider.calls.lock().unwrap(), 2);
        assert_eq!(provider.rotated_key.lock().unwrap().as_deref(), Some("second-key"));
    }

    #[tokio::test]
    async fn content_policy_failure_is_not_retried() {
        #[derive(Clone)]
        struct ContentPolicyProvider {
            calls: Arc<Mutex<usize>>,
        }

        #[async_trait]
        impl Provider for ContentPolicyProvider {
            fn name(&self) -> &str {
                "content-policy"
            }
            fn default_model(&self) -> &str {
                "model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                *self.calls.lock().unwrap() += 1;
                Err(crate::error::SyscityError::ExternalService {
                    source: "OpenAI API error 400: Content policy violation".into(),
                    cause: None,
                })
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                Ok(true)
            }
            async fn set_credential(
                &self,
                _credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        let router = ModelRouter::new(ModelRouterConfig::default());
        let provider = Arc::new(ContentPolicyProvider { calls: Arc::new(Mutex::new(0)) });
        router
            .add_provider_instance("content-policy", provider.clone())
            .await
            .unwrap();

        router
            .set_alias(ModelAlias {
                name: "test".to_string(),
                provider: "content-policy".to_string(),
                model: "model".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let result = router
            .complete("test", vec![Message::user("hi")], None)
            .await;
        assert!(result.is_err());
        assert_eq!(*provider.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn concurrent_complete_calls_do_not_deadlock() {
        #[derive(Clone)]
        struct SlowProvider {
            calls: Arc<Mutex<usize>>,
        }

        #[async_trait]
        impl Provider for SlowProvider {
            fn name(&self) -> &str {
                "slow"
            }
            fn default_model(&self) -> &str {
                "slow-model"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            fn max_context(&self) -> usize {
                4096
            }
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> crate::Result<CompletionResponse> {
                tokio::time::sleep(Duration::from_millis(50)).await;
                *self.calls.lock().unwrap() += 1;
                Ok(CompletionResponse {
                    message: Message::assistant("ok"),
                    model: "slow-model".to_string(),
                    usage: Some(Usage::default()),
                    finish_reason: Some("stop".to_string()),
                })
            }
            async fn stream(&self, _request: CompletionRequest) -> crate::Result<CompletionStream> {
                unimplemented!()
            }
            async fn health_check(&self) -> crate::Result<bool> {
                Ok(true)
            }
            async fn set_credential(
                &self,
                _credential: crate::model_router::Credential,
            ) -> crate::Result<()> {
                Ok(())
            }
        }

        let router = Arc::new(ModelRouter::new(ModelRouterConfig::default()));
        let provider = Arc::new(SlowProvider { calls: Arc::new(Mutex::new(0)) });
        router
            .add_provider_instance("slow", provider.clone())
            .await
            .unwrap();
        router
            .set_alias(ModelAlias {
                name: "test".to_string(),
                provider: "slow".to_string(),
                model: "slow-model".to_string(),
                temperature: None,
                max_tokens: None,
            })
            .await;

        let r1 = router.clone();
        let r2 = router.clone();
        let (first, second) = tokio::join!(
            tokio::time::timeout(Duration::from_secs(2), async move {
                r1.complete("test", vec![Message::user("a")], None).await
            }),
            tokio::time::timeout(Duration::from_secs(2), async move {
                r2.complete("test", vec![Message::user("b")], None).await
            }),
        );

        assert!(first.is_ok(), "first complete timed out (possible deadlock)");
        assert!(second.is_ok(), "second complete timed out (possible deadlock)");
        assert!(first.unwrap().is_ok());
        assert!(second.unwrap().is_ok());
        assert_eq!(*provider.calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn all_snapshots_with_quota_fetches_for_all_providers() {
        #[derive(Clone)]
        struct MockUsageFetcher {
            remaining: f64,
        }

        #[async_trait]
        impl UsageFetcher for MockUsageFetcher {
            fn provider(&self) -> &str {
                "mock"
            }

            async fn fetch(&self) -> crate::Result<Option<UsageQuota>> {
                Ok(Some(UsageQuota {
                    remaining: self.remaining,
                    limit: 100.0,
                    reset_at: None,
                    unit: "usd".to_string(),
                    source: QuotaSource::Remote,
                }))
            }
        }

        let router = ModelRouter::new(ModelRouterConfig::default());
        let usage = Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        };
        router.usage_tracker.record("openai", usage, "gpt-4o").await;
        router
            .usage_tracker
            .record("anthropic", usage, "claude-3-opus")
            .await;

        {
            let mut fetchers = router.usage_fetchers.write().await;
            fetchers.register("openai", Arc::new(MockUsageFetcher { remaining: 42.0 }));
            fetchers.register("anthropic", Arc::new(MockUsageFetcher { remaining: 7.0 }));
        }

        let snapshots = router.all_snapshots_with_quota().await;
        assert_eq!(snapshots.len(), 2);

        let openai = snapshots.iter().find(|s| s.provider == "openai").unwrap();
        let anthropic = snapshots
            .iter()
            .find(|s| s.provider == "anthropic")
            .unwrap();

        assert_eq!(openai.quota.as_ref().unwrap().remaining, 42.0);
        assert_eq!(anthropic.quota.as_ref().unwrap().remaining, 7.0);
    }
}
