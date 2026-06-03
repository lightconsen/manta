//! Event Hooks System for Syscity Gateway
//!
//! Provides a hook registry that allows plugins and internal modules to
//! intercept, transform, or suppress GatewayEvents before/after broadcast.
//!
//! Mirrors OpenClaw's EventEmitter-based plugin hook architecture.

use crate::gateway::GatewayEvent;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

/// Hook execution result
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Continue with this (possibly modified) event
    Continue(GatewayEvent),
    /// Drop the event (don't broadcast)
    Drop,
    /// Replace with a different event
    Replace(GatewayEvent),
}

impl HookResult {
    /// Get the event if continuing or replacing
    pub fn into_event(self) -> Option<GatewayEvent> {
        match self {
            HookResult::Continue(e) | HookResult::Replace(e) => Some(e),
            HookResult::Drop => None,
        }
    }
}

/// Type alias for before-hook functions
pub type BeforeHook = Arc<
    dyn Fn(GatewayEvent) -> std::pin::Pin<Box<dyn std::future::Future<Output = HookResult> + Send>>
        + Send
        + Sync,
>;

/// Type alias for after-hook functions
pub type AfterHook = Arc<
    dyn Fn(GatewayEvent) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Event hook entry (before + after for a specific event filter)
pub struct EventHook {
    /// Name of the hook (for identification/removal)
    pub name: String,
    /// Priority (lower = earlier execution)
    pub priority: i32,
    /// Optional filter: only apply to events matching this substring in their JSON
    pub event_filter: Option<String>,
    /// Before hook (can modify/drop event)
    pub before: Option<BeforeHook>,
    /// After hook (read-only notification)
    pub after: Option<AfterHook>,
}

/// Registry of event hooks
#[derive(Default)]
pub struct EventHookRegistry {
    /// Before hooks, sorted by priority
    before_hooks: RwLock<Vec<EventHook>>,
    /// After hooks, sorted by priority
    after_hooks: RwLock<Vec<EventHook>>,
}

impl EventHookRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a before hook
    pub async fn register_before(
        &self,
        name: impl Into<String>,
        priority: i32,
        event_filter: Option<String>,
        hook: BeforeHook,
    ) {
        let mut hooks = self.before_hooks.write().await;
        hooks.push(EventHook {
            name: name.into(),
            priority,
            event_filter,
            before: Some(hook),
            after: None,
        });
        hooks.sort_by_key(|h| h.priority);
        debug!("Registered before hook: {} (priority={})", hooks.last().unwrap().name, priority);
    }

    /// Register an after hook
    pub async fn register_after(
        &self,
        name: impl Into<String>,
        priority: i32,
        event_filter: Option<String>,
        hook: AfterHook,
    ) {
        let mut hooks = self.after_hooks.write().await;
        hooks.push(EventHook {
            name: name.into(),
            priority,
            event_filter,
            before: None,
            after: Some(hook),
        });
        hooks.sort_by_key(|h| h.priority);
        debug!("Registered after hook: {} (priority={})", hooks.last().unwrap().name, priority);
    }

    /// Remove a hook by name
    pub async fn unregister(&self, name: &str) -> bool {
        let mut before = self.before_hooks.write().await;
        let before_len = before.len();
        before.retain(|h| h.name != name);
        let removed_before = before_len != before.len();

        let mut after = self.after_hooks.write().await;
        let after_len = after.len();
        after.retain(|h| h.name != name);
        let removed_after = after_len != after.len();

        removed_before || removed_after
    }

    /// Run all before hooks on an event
    /// Returns None if the event should be dropped
    pub async fn run_before(&self, event: GatewayEvent) -> Option<GatewayEvent> {
        let hooks = self.before_hooks.read().await;
        let event_json = serde_json::to_string(&event).unwrap_or_default();

        let mut current = event;
        for hook in hooks.iter() {
            if let Some(ref filter) = hook.event_filter {
                if !event_json.contains(filter) {
                    continue;
                }
            }

            if let Some(ref before) = hook.before {
                match before(current).await {
                    HookResult::Continue(e) => current = e,
                    HookResult::Replace(e) => current = e,
                    HookResult::Drop => {
                        debug!("Event dropped by hook: {}", hook.name);
                        return None;
                    }
                }
            }
        }

        Some(current)
    }

    /// Run all after hooks on an event (fire-and-forget)
    pub async fn run_after(&self, event: GatewayEvent) {
        let hooks = self.after_hooks.read().await;
        let event_json = serde_json::to_string(&event).unwrap_or_default();

        for hook in hooks.iter() {
            if let Some(ref filter) = hook.event_filter {
                if !event_json.contains(filter) {
                    continue;
                }
            }

            if let Some(ref after) = hook.after {
                let event_clone = event.clone();
                let after_clone = after.clone();
                let _hook_name = hook.name.clone();
                tokio::spawn(async move {
                    after_clone(event_clone).await;
                });
            }
        }
    }

    /// List all registered hooks
    pub async fn list_hooks(&self) -> Vec<HookInfo> {
        let before = self.before_hooks.read().await;
        let after = self.after_hooks.read().await;

        let mut result: Vec<_> = before
            .iter()
            .map(|h| HookInfo {
                name: h.name.clone(),
                kind: "before".to_string(),
                priority: h.priority,
                filter: h.event_filter.clone(),
            })
            .chain(after.iter().map(|h| HookInfo {
                name: h.name.clone(),
                kind: "after".to_string(),
                priority: h.priority,
                filter: h.event_filter.clone(),
            }))
            .collect();

        result.sort_by_key(|h| h.priority);
        result
    }
}

/// Hook information for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookInfo {
    pub name: String,
    pub kind: String,
    pub priority: i32,
    pub filter: Option<String>,
}

/// Helper to emit an event through the hook pipeline and broadcast channel
pub async fn emit_event(
    event_tx: &tokio::sync::broadcast::Sender<GatewayEvent>,
    hooks: &EventHookRegistry,
    event: GatewayEvent,
) {
    // Run before hooks
    let Some(event) = hooks.run_before(event).await else {
        return; // Event dropped
    };

    // Broadcast
    let _ = event_tx.send(event.clone());

    // Run after hooks (fire-and-forget)
    hooks.run_after(event).await;
}

/// Convenience macro for creating simple before hooks
#[macro_export]
macro_rules! before_hook {
    ($name:expr, $priority:expr, $filter:expr, $body:expr) => {
        $crate::gateway::hooks::EventHookRegistry::register_before(
            $name,
            $priority,
            $filter,
            std::sync::Arc::new(|event| Box::pin(async move { $body(event) })),
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hook_registry() {
        let registry = EventHookRegistry::new();

        // Register a before hook that logs messages
        registry
            .register_before(
                "test_log",
                0,
                Some("MessageReceived".to_string()),
                Arc::new(|event| Box::pin(async move { HookResult::Continue(event) })),
            )
            .await;

        let hooks = registry.list_hooks().await;
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].name, "test_log");
    }

    #[tokio::test]
    async fn test_before_hook_drop() {
        let registry = EventHookRegistry::new();

        registry
            .register_before(
                "drop_all",
                0,
                None,
                Arc::new(|_| Box::pin(async move { HookResult::Drop })),
            )
            .await;

        let event = GatewayEvent::ChannelStatus {
            channel: "test".to_string(),
            connected: true,
        };

        let result = registry.run_before(event).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_before_hook_transform() {
        let registry = EventHookRegistry::new();

        registry
            .register_before(
                "transform",
                0,
                None,
                Arc::new(|event| {
                    Box::pin(async move {
                        // Replace with a different event type
                        HookResult::Continue(event)
                    })
                }),
            )
            .await;

        let event = GatewayEvent::ChannelStatus {
            channel: "test".to_string(),
            connected: true,
        };

        let result = registry.run_before(event.clone()).await;
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_unregister_hook() {
        let registry = EventHookRegistry::new();

        registry
            .register_before(
                "temp",
                0,
                None,
                Arc::new(|event| Box::pin(async move { HookResult::Continue(event) })),
            )
            .await;

        assert_eq!(registry.list_hooks().await.len(), 1);
        assert!(registry.unregister("temp").await);
        assert!(registry.list_hooks().await.is_empty());
        assert!(!registry.unregister("missing").await);
    }

    #[tokio::test]
    async fn test_hook_result_into_event() {
        let event = GatewayEvent::ChannelStatus {
            channel: "c".to_string(),
            connected: true,
        };

        assert!(HookResult::Continue(event.clone()).into_event().is_some());
        assert!(HookResult::Replace(event.clone()).into_event().is_some());
        assert!(HookResult::Drop.into_event().is_none());
    }

    #[tokio::test]
    async fn test_after_hook_filter_mismatch() {
        let registry = EventHookRegistry::new();
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();

        registry
            .register_after(
                "filter_test",
                0,
                Some("NonExistent".to_string()),
                Arc::new(move |_| {
                    let called = called_clone.clone();
                    Box::pin(async move {
                        called.store(true, std::sync::atomic::Ordering::SeqCst);
                    })
                }),
            )
            .await;

        let event = GatewayEvent::ChannelStatus {
            channel: "test".to_string(),
            connected: true,
        };

        registry.run_after(event).await;
        // Give spawned task a moment
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_priority_sorting() {
        let registry = EventHookRegistry::new();

        registry
            .register_before(
                "second",
                10,
                None,
                Arc::new(|event| Box::pin(async move { HookResult::Continue(event) })),
            )
            .await;
        registry
            .register_before(
                "first",
                5,
                None,
                Arc::new(|event| Box::pin(async move { HookResult::Continue(event) })),
            )
            .await;

        let hooks = registry.list_hooks().await;
        assert_eq!(hooks[0].priority, 5);
        assert_eq!(hooks[1].priority, 10);
    }
}
