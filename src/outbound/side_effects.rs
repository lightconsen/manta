//! Side Effects Executor
//!
//! Executes post-response side effects: memory storage, cron scheduling,
//! webhook triggers, analytics logging, etc.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// A declarative side effect.
#[derive(Debug, Clone)]
pub enum SideEffect {
    /// Store a memory entry.
    MemoryStore { session_id: String, content: String },
    /// Schedule a cron job.
    CronSchedule { expression: String, payload: String },
    /// Trigger a webhook.
    Webhook {
        url: String,
        payload: serde_json::Value,
    },
    /// Log an analytics event.
    Analytics {
        event: String,
        properties: HashMap<String, serde_json::Value>,
    },
    /// Custom side effect (for plugins).
    Custom {
        name: String,
        params: serde_json::Value,
    },
}

/// Side effect handler trait.
#[async_trait::async_trait]
pub trait SideEffectHandler: Send + Sync {
    /// Unique name for this handler.
    fn name(&self) -> &str;
    /// Execute the side effect.
    async fn execute(&self, effect: &SideEffect) -> Result<(), SideEffectError>;
}

/// Registry of side effect handlers.
pub struct SideEffectRegistry {
    handlers: RwLock<HashMap<String, Arc<dyn SideEffectHandler>>>,
}

impl SideEffectRegistry {
    pub fn new() -> Self {
        Self {
            handlers: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register(&self, handler: Arc<dyn SideEffectHandler>) {
        let mut handlers = self.handlers.write().await;
        info!("Registered side-effect handler: {}", handler.name());
        handlers.insert(handler.name().to_string(), handler);
    }

    pub async fn get(&self, name: &str) -> Option<Arc<dyn SideEffectHandler>> {
        let handlers = self.handlers.read().await;
        handlers.get(name).cloned()
    }
}

impl Default for SideEffectRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared context for executing built-in side effects.
/// Populated by the gateway after state initialization.
#[derive(Debug, Clone, Default)]
pub struct SideEffectContext {
    /// Memory manager for MemoryStore effects.
    pub memory_manager: Option<Arc<crate::memory::MemoryManager>>,
    /// Cron scheduler for CronSchedule effects.
    pub cron_scheduler: Option<Arc<tokio::sync::Mutex<crate::cron::cron::CronScheduler>>>,
    /// Webhook client for Webhook effects.
    pub webhook_client: Option<Arc<reqwest::Client>>,
}

/// Executor that runs side effects asynchronously.
pub struct SideEffectExecutor {
    registry: Arc<SideEffectRegistry>,
    /// Shared context populated at runtime by the gateway.
    ctx: RwLock<SideEffectContext>,
}

impl SideEffectExecutor {
    pub fn new(registry: Arc<SideEffectRegistry>) -> Self {
        Self {
            registry,
            ctx: RwLock::new(SideEffectContext::default()),
        }
    }

    /// Set the runtime context (called by Gateway after state init).
    pub async fn set_context(&self, ctx: SideEffectContext) {
        let mut guard = self.ctx.write().await;
        *guard = ctx;
    }

    /// Execute a batch of side effects.
    ///
    /// Errors are logged but do not fail the whole batch.
    pub async fn execute_batch(&self, effects: &[SideEffect]) {
        for effect in effects {
            self.execute_one(effect).await;
        }
    }

    async fn execute_one(&self, effect: &SideEffect) {
        let ctx = self.ctx.read().await.clone();

        match effect {
            SideEffect::MemoryStore { session_id, content } => {
                if let Some(ref mm) = ctx.memory_manager {
                    match mm
                        .observe(session_id, content.clone(), "side_effect", 0.5)
                        .await
                    {
                        Ok(id) => {
                            debug!("MemoryStore: saved entry {} for session {}", id, session_id)
                        }
                        Err(e) => error!("MemoryStore side-effect failed: {}", e),
                    }
                } else {
                    warn!("MemoryStore side-effect: no memory manager configured, effect dropped");
                }
            }

            SideEffect::CronSchedule { expression, payload } => {
                if let Some(ref scheduler) = ctx.cron_scheduler {
                    let schedule = crate::cron::cron::Schedule::Cron {
                        expression: expression.clone(),
                        timezone: None,
                        stagger_ms: None,
                    };
                    let target = crate::cron::cron::ExecutionTarget::shell(payload.clone());
                    let job = crate::cron::cron::CronJob::new(
                        uuid::Uuid::new_v4().to_string(),
                        format!("side-effect-{}", expression),
                        schedule,
                        target,
                    );
                    let guard = scheduler.lock().await;
                    if let Err(e) = guard.add_job(job).await {
                        error!("CronSchedule side-effect failed: {}", e);
                    } else {
                        debug!("CronSchedule: added job for '{}'", expression);
                    }
                } else {
                    warn!("CronSchedule side-effect: no cron scheduler configured, effect dropped");
                }
            }

            SideEffect::Webhook { url, payload } => {
                let client = match &ctx.webhook_client {
                    Some(c) => c.clone(),
                    None => {
                        debug!(
                            "Webhook side-effect: no shared client configured, creating one-off"
                        );
                        Arc::new(
                            reqwest::Client::builder()
                                .timeout(std::time::Duration::from_secs(10))
                                .build()
                                .unwrap_or_default(),
                        )
                    }
                };
                let url = url.clone();
                let payload = payload.clone();
                tokio::spawn(async move {
                    match client.post(&url).json(&payload).send().await {
                        Ok(resp) => {
                            let status = resp.status();
                            // Drain the response body to free the connection pool slot.
                            let _ = resp.bytes().await;
                            debug!("Webhook side-effect: {} status={}", url, status);
                        }
                        Err(e) => error!("Webhook side-effect failed: {} {}", url, e),
                    }
                });
            }

            SideEffect::Analytics { event, properties } => {
                info!(
                    event = %event,
                    properties = ?properties,
                    "Analytics side-effect"
                );
            }

            SideEffect::Custom { name, params: _params } => {
                if let Some(handler) = self.registry.get(name).await {
                    if let Err(e) = handler.execute(effect).await {
                        error!("Custom side-effect '{}' failed: {}", name, e);
                    }
                } else {
                    warn!("No handler registered for custom side-effect: {}", name);
                }
            }
        }
    }
}

/// Errors from side effect execution.
#[derive(Debug, thiserror::Error)]
pub enum SideEffectError {
    #[error("Handler not found: {0}")]
    HandlerNotFound(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHandler {
        name: String,
    }

    #[async_trait::async_trait]
    impl SideEffectHandler for TestHandler {
        fn name(&self) -> &str {
            &self.name
        }
        async fn execute(&self, _effect: &SideEffect) -> Result<(), SideEffectError> {
            Ok(())
        }
    }

    struct FailingHandler;

    #[async_trait::async_trait]
    impl SideEffectHandler for FailingHandler {
        fn name(&self) -> &str {
            "failing"
        }
        async fn execute(&self, _effect: &SideEffect) -> Result<(), SideEffectError> {
            Err(SideEffectError::ExecutionFailed("boom".to_string()))
        }
    }

    #[tokio::test]
    async fn test_registry_register_and_get() {
        let registry = Arc::new(SideEffectRegistry::new());

        registry
            .register(Arc::new(TestHandler { name: "test".to_string() }))
            .await;
        assert!(registry.get("test").await.is_some());
        assert!(registry.get("missing").await.is_none());
    }

    #[tokio::test]
    async fn test_registry_default() {
        let registry: SideEffectRegistry = Default::default();
        assert!(registry.get("anything").await.is_none());
    }

    #[tokio::test]
    async fn test_execute_batch_memory() {
        let registry = Arc::new(SideEffectRegistry::new());
        let executor = SideEffectExecutor::new(registry);

        let effects = vec![SideEffect::MemoryStore {
            session_id: "s1".to_string(),
            content: "hello".to_string(),
        }];

        executor.execute_batch(&effects).await;
    }

    #[tokio::test]
    async fn test_execute_analytics_no_panic() {
        let registry = Arc::new(SideEffectRegistry::new());
        let executor = SideEffectExecutor::new(registry);

        let effect = SideEffect::Analytics {
            event: "test".to_string(),
            properties: {
                let mut map = HashMap::new();
                map.insert("key".to_string(), serde_json::json!("value"));
                map
            },
        };

        executor.execute_one(&effect).await;
    }

    #[tokio::test]
    async fn test_execute_cron_without_scheduler() {
        let registry = Arc::new(SideEffectRegistry::new());
        let executor = SideEffectExecutor::new(registry);

        let effect = SideEffect::CronSchedule {
            expression: "0 * * * *".to_string(),
            payload: "echo hello".to_string(),
        };

        executor.execute_one(&effect).await;
    }

    #[tokio::test]
    async fn test_execute_webhook_without_client() {
        let registry = Arc::new(SideEffectRegistry::new());
        let executor = SideEffectExecutor::new(registry);

        let effect = SideEffect::Webhook {
            url: "http://localhost:9999".to_string(),
            payload: serde_json::json!({"test": true}),
        };

        executor.execute_one(&effect).await;
    }

    #[tokio::test]
    async fn test_execute_custom_with_handler() {
        let registry = Arc::new(SideEffectRegistry::new());
        registry
            .register(Arc::new(TestHandler { name: "custom".to_string() }))
            .await;

        let executor = SideEffectExecutor::new(registry);

        let effect = SideEffect::Custom {
            name: "custom".to_string(),
            params: serde_json::json!({}),
        };

        executor.execute_one(&effect).await;
    }

    #[tokio::test]
    async fn test_execute_custom_without_handler() {
        let registry = Arc::new(SideEffectRegistry::new());
        let executor = SideEffectExecutor::new(registry);

        let effect = SideEffect::Custom {
            name: "missing".to_string(),
            params: serde_json::json!({}),
        };

        executor.execute_one(&effect).await;
    }

    #[tokio::test]
    async fn test_execute_custom_with_failing_handler() {
        let registry = Arc::new(SideEffectRegistry::new());
        registry.register(Arc::new(FailingHandler)).await;

        let executor = SideEffectExecutor::new(registry);

        let effect = SideEffect::Custom {
            name: "failing".to_string(),
            params: serde_json::json!({}),
        };

        executor.execute_one(&effect).await;
    }

    #[tokio::test]
    async fn test_executor_set_context() {
        let registry = Arc::new(SideEffectRegistry::new());
        let executor = SideEffectExecutor::new(registry);

        let ctx = SideEffectContext {
            memory_manager: None,
            cron_scheduler: None,
            webhook_client: None,
        };
        executor.set_context(ctx).await;
    }

    #[test]
    fn test_side_effect_debug() {
        let effect = SideEffect::MemoryStore {
            session_id: "s1".to_string(),
            content: "hello".to_string(),
        };
        let debug = format!("{:?}", effect);
        assert!(debug.contains("MemoryStore"));
    }

    #[test]
    fn test_side_effect_error_display() {
        let err = SideEffectError::HandlerNotFound("foo".to_string());
        assert_eq!(err.to_string(), "Handler not found: foo");

        let err = SideEffectError::ExecutionFailed("bar".to_string());
        assert_eq!(err.to_string(), "Execution failed: bar");
    }

    #[test]
    fn test_side_effect_context_default() {
        let ctx = SideEffectContext::default();
        assert!(ctx.memory_manager.is_none());
        assert!(ctx.cron_scheduler.is_none());
        assert!(ctx.webhook_client.is_none());
    }

    #[tokio::test]
    async fn test_execute_batch_empty() {
        let registry = Arc::new(SideEffectRegistry::new());
        let executor = SideEffectExecutor::new(registry);
        executor.execute_batch(&[]).await;
    }

    #[tokio::test]
    async fn test_execute_memory_without_manager() {
        let registry = Arc::new(SideEffectRegistry::new());
        let executor = SideEffectExecutor::new(registry);

        let effect = SideEffect::MemoryStore {
            session_id: "s1".to_string(),
            content: "hello".to_string(),
        };

        executor.execute_one(&effect).await;
    }
}
