//! Side Effects Executor
//!
//! Executes post-response side effects: memory storage, cron scheduling,
//! webhook triggers, analytics logging, etc.
//!
//! Design matches OpenClaw's post-response hook system.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// A declarative side effect.
#[derive(Debug, Clone)]
pub enum SideEffect {
    /// Store a memory entry.
    MemoryStore {
        session_id: String,
        content: String,
        tags: Vec<String>,
    },
    /// Schedule a cron job.
    CronSchedule {
        expression: String,
        payload: String,
    },
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

    pub async fn register(&self,
        handler: Arc<dyn SideEffectHandler>,
    ) {
        let mut handlers = self.handlers.write().await;
        info!("Registered side-effect handler: {}", handler.name());
        handlers.insert(handler.name().to_string(), handler);
    }

    pub async fn get(&self,
        name: &str,
    ) -> Option<Arc<dyn SideEffectHandler>> {
        let handlers = self.handlers.read().await;
        handlers.get(name).cloned()
    }
}

impl Default for SideEffectRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Executor that runs side effects asynchronously.
pub struct SideEffectExecutor {
    registry: Arc<SideEffectRegistry>,
}

impl SideEffectExecutor {
    pub fn new(registry: Arc<SideEffectRegistry>) -> Self {
        Self { registry }
    }

    /// Execute a batch of side effects.
    ///
    /// Errors are logged but do not fail the whole batch.
    pub async fn execute_batch(&self,
        effects: &[SideEffect],
    ) {
        for effect in effects {
            match effect {
                SideEffect::MemoryStore { session_id, content: _content, tags } => {
                    debug!(
                        "Side effect: memory store for session {} (tags: {:?})",
                        session_id, tags
                    );
                    // Stub: would call MemoryManager::store()
                }
                SideEffect::CronSchedule { expression, payload } => {
                    debug!(
                        "Side effect: cron schedule '{}' with payload {}",
                        expression, payload
                    );
                    // Stub: would call CronScheduler::add()
                }
                SideEffect::Webhook { url, payload } => {
                    debug!(
                        "Side effect: webhook to {} with payload {:?}",
                        url, payload
                    );
                    // Stub: would fire HTTP POST
                }
                SideEffect::Analytics { event, properties } => {
                    debug!(
                        "Side effect: analytics event {} with {:?}",
                        event, properties
                    );
                    // Stub: would send to analytics backend
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

    #[tokio::test]
    async fn test_registry_register_and_get() {
        let registry = Arc::new(SideEffectRegistry::new());

        struct TestHandler;
        #[async_trait::async_trait]
        impl SideEffectHandler for TestHandler {
            fn name(&self) -> &str { "test" }
            async fn execute(&self, _effect: &SideEffect) -> Result<(), SideEffectError> {
                Ok(())
            }
        }

        registry.register(Arc::new(TestHandler)).await;
        assert!(registry.get("test").await.is_some());
        assert!(registry.get("missing").await.is_none());
    }

    #[tokio::test]
    async fn test_execute_batch_memory() {
        let registry = Arc::new(SideEffectRegistry::new());
        let executor = SideEffectExecutor::new(registry);

        let effects = vec![
            SideEffect::MemoryStore {
                session_id: "s1".to_string(),
                content: "hello".to_string(),
                tags: vec!["greeting".to_string()],
            },
        ];

        executor.execute_batch(&effects).await;
    }
}
