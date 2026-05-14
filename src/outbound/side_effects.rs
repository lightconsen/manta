//! Side Effects Executor
//!
//! Executes post-response side effects: memory storage, cron scheduling,
//! webhook triggers, analytics logging, etc.
//!
//! Design matches OpenClaw's post-response hook system.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
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
    /// Sender for offloading effect execution to a background task.
    effect_tx: mpsc::Sender<SideEffect>,
}

impl SideEffectExecutor {
    pub fn new(registry: Arc<SideEffectRegistry>) -> Self {
        let (effect_tx, _effect_rx) = mpsc::channel(256);
        Self {
            registry,
            ctx: RwLock::new(SideEffectContext::default()),
            effect_tx,
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
            SideEffect::MemoryStore {
                session_id,
                content,
                tags: _tags,
            } => {
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
                    debug!("MemoryStore: no memory manager configured");
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
                    debug!("CronSchedule: no cron scheduler configured");
                }
            }

            SideEffect::Webhook { url, payload } => {
                let client = ctx.webhook_client.unwrap_or_else(|| {
                    Arc::new(
                        reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(10))
                            .build()
                            .unwrap_or_default(),
                    )
                });
                let url = url.clone();
                let payload = payload.clone();
                tokio::spawn(async move {
                    match client.post(&url).json(&payload).send().await {
                        Ok(resp) => debug!("Webhook side-effect: {} {}", url, resp.status()),
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

    #[tokio::test]
    async fn test_registry_register_and_get() {
        let registry = Arc::new(SideEffectRegistry::new());

        struct TestHandler;
        #[async_trait::async_trait]
        impl SideEffectHandler for TestHandler {
            fn name(&self) -> &str {
                "test"
            }
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

        let effects = vec![SideEffect::MemoryStore {
            session_id: "s1".to_string(),
            content: "hello".to_string(),
            tags: vec!["greeting".to_string()],
        }];

        executor.execute_batch(&effects).await;
    }
}
