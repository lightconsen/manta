//! Tool execution hooks
//!
//! Provides pre- and post-execution hooks for tools, enabling audit logging,
//! permission checks, metrics collection, and result caching at the call site.

use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::ToolExecutionResult;

/// A boxed async function called before a tool executes.
///
/// Receives the tool name and the arguments that will be passed to the tool.
pub type BeforeHookFn =
    Arc<dyn Fn(&str, &Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A boxed async function called after a tool executes.
///
/// Receives the tool name, the original arguments, and the execution result.
pub type AfterHookFn = Arc<
    dyn Fn(&str, &Value, &ToolExecutionResult) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// A collection of before/after hooks for tool execution.
///
/// Hooks are opt-in and layered: all registered hooks run in registration order.
///
/// # Example
///
/// ```rust,no_run
/// use manta::tools::hooks::ToolHooks;
///
/// let hooks = ToolHooks::new()
///     .before(|name, args| {
///         let name = name.to_string();
///         let args = args.to_string(); // stringify before entering the async block
///         Box::pin(async move {
///             tracing::info!("Calling tool: {} with args: {}", name, args);
///         })
///     })
///     .after(|name, _args, result| {
///         let name = name.to_string();
///         let success = result.success;
///         Box::pin(async move {
///             tracing::info!("Tool {} completed, success={}", name, success);
///         })
///     });
/// ```
#[derive(Default, Clone)]
pub struct ToolHooks {
    before_call: Vec<BeforeHookFn>,
    after_call: Vec<AfterHookFn>,
}

impl std::fmt::Debug for ToolHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolHooks")
            .field("before_hooks", &self.before_call.len())
            .field("after_hooks", &self.after_call.len())
            .finish()
    }
}

impl ToolHooks {
    /// Create a new empty `ToolHooks`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a hook to run before tool execution.
    ///
    /// The hook receives the tool name and the call arguments.
    pub fn before<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(&str, &Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.before_call.push(Arc::new(move |name, args| {
            Box::pin(f(name, args)) as Pin<Box<dyn Future<Output = ()> + Send>>
        }));
        self
    }

    /// Add a hook to run after tool execution.
    ///
    /// The hook receives the tool name, the original arguments, and the result.
    pub fn after<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(&str, &Value, &ToolExecutionResult) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.after_call.push(Arc::new(move |name, args, result| {
            Box::pin(f(name, args, result)) as Pin<Box<dyn Future<Output = ()> + Send>>
        }));
        self
    }

    /// Returns `true` if no hooks are registered.
    pub fn is_empty(&self) -> bool {
        self.before_call.is_empty() && self.after_call.is_empty()
    }

    /// Run all registered before-hooks for the given tool call.
    pub async fn run_before(&self, name: &str, args: &Value) {
        for hook in &self.before_call {
            hook(name, args).await;
        }
    }

    /// Run all registered after-hooks for the given tool call.
    pub async fn run_after(&self, name: &str, args: &Value, result: &ToolExecutionResult) {
        for hook in &self.after_call {
            hook(name, args, result).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_before_hook_called() {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);

        let hooks = ToolHooks::new().before(move |_name, _args| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        });

        hooks.run_before("shell", &serde_json::json!({})).await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_after_hook_called() {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);

        let hooks = ToolHooks::new().after(move |_name, _args, _result| {
            let c = Arc::clone(&c);
            async move {
                c.fetch_add(1, Ordering::SeqCst);
            }
        });

        let result = ToolExecutionResult::success("ok".to_string());
        hooks
            .run_after("shell", &serde_json::json!({}), &result)
            .await;
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_multiple_hooks_run_in_order() {
        let log: Arc<tokio::sync::Mutex<Vec<u32>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));

        let l1 = Arc::clone(&log);
        let l2 = Arc::clone(&log);

        let hooks = ToolHooks::new()
            .before(move |_, _| {
                let l = Arc::clone(&l1);
                async move {
                    l.lock().await.push(1);
                }
            })
            .before(move |_, _| {
                let l = Arc::clone(&l2);
                async move {
                    l.lock().await.push(2);
                }
            });

        hooks.run_before("tool", &serde_json::json!({})).await;
        let order = log.lock().await.clone();
        assert_eq!(order, vec![1, 2]);
    }

    #[test]
    fn test_is_empty() {
        let hooks = ToolHooks::new();
        assert!(hooks.is_empty());

        let hooks = hooks.before(|_, _| async {});
        assert!(!hooks.is_empty());
    }
}
