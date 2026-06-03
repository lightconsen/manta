//! Plugin Hooks System
//!
//! Allows plugins to hook into various events and extend behavior.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Hook types that plugins can subscribe to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookType {
    /// Called before a message is processed
    BeforeMessageProcess,
    /// Called after a message is processed
    AfterMessageProcess,
    /// Called before a tool is executed
    BeforeToolExecute,
    /// Called after a tool is executed
    AfterToolExecute,
    /// Called when a new session starts
    SessionStart,
    /// Called when a session ends
    SessionEnd,
    /// Called when configuration is loaded
    ConfigLoad,
    /// Called before provider call
    BeforeProviderCall,
    /// Called after provider call
    AfterProviderCall,
}

impl std::fmt::Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            HookType::BeforeMessageProcess => "before_message_process",
            HookType::AfterMessageProcess => "after_message_process",
            HookType::BeforeToolExecute => "before_tool_execute",
            HookType::AfterToolExecute => "after_tool_execute",
            HookType::SessionStart => "session_start",
            HookType::SessionEnd => "session_end",
            HookType::ConfigLoad => "config_load",
            HookType::BeforeProviderCall => "before_provider_call",
            HookType::AfterProviderCall => "after_provider_call",
        };
        write!(f, "{}", s)
    }
}

/// Hook payload - data passed to hook handlers
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPayload {
    /// Message processing data
    MessageProcess {
        session_id: String,
        user_id: String,
        content: String,
        channel: String,
    },
    /// Tool execution data
    ToolExecute {
        tool_name: String,
        parameters: serde_json::Value,
        result: Option<serde_json::Value>,
    },
    /// Session lifecycle data
    Session {
        session_id: String,
        user_id: String,
        agent_id: Option<String>,
    },
    /// Configuration data
    Config {
        config_path: Option<String>,
        config_data: serde_json::Value,
    },
    /// Provider call data
    ProviderCall {
        provider: String,
        model: Option<String>,
        messages: Vec<serde_json::Value>,
        response: Option<String>,
    },
}

/// Hook result - what the hook handler returns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookResult {
    /// Continue with default behavior
    Continue,
    /// Modify the payload and continue
    Modify(HookPayload),
    /// Cancel the operation (for before hooks)
    Cancel { reason: String },
    /// Error occurred
    Error { message: String },
}

/// A registered hook handler
#[derive(Clone)]
pub struct HookHandler {
    /// Plugin ID that registered this handler
    pub plugin_id: String,
    /// Hook type
    pub hook_type: HookType,
    /// Handler priority (lower = earlier)
    pub priority: i32,
    /// Handler function
    pub handler:
        Arc<dyn Fn(HookPayload) -> futures::future::BoxFuture<'static, HookResult> + Send + Sync>,
}

/// Hook registry - manages all hook handlers
pub struct HookRegistry {
    handlers: Arc<RwLock<HashMap<HookType, Vec<HookHandler>>>>,
}

impl HookRegistry {
    /// Create a new hook registry
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a hook handler
    pub async fn register(&self, handler: HookHandler) {
        let mut handlers = self.handlers.write().await;
        let entry = handlers.entry(handler.hook_type).or_default();
        entry.push(handler);
        // Sort by priority
        entry.sort_by_key(|h| h.priority);
    }

    /// Unregister all handlers for a plugin
    pub async fn unregister_plugin(&self, plugin_id: &str) {
        let mut handlers = self.handlers.write().await;
        for handlers_list in handlers.values_mut() {
            handlers_list.retain(|h| h.plugin_id != plugin_id);
        }
    }

    /// Execute hooks for a given type
    pub async fn execute(&self, hook_type: HookType, payload: HookPayload) -> HookExecutionResult {
        let handlers = self.handlers.read().await;
        let Some(handlers_list) = handlers.get(&hook_type) else {
            return HookExecutionResult::Continue(payload);
        };

        let mut current_payload = payload;

        for handler in handlers_list {
            debug!("Executing hook {:?} for plugin '{}'", hook_type, handler.plugin_id);

            let result = (handler.handler)(current_payload.clone()).await;

            match result {
                HookResult::Continue => continue,
                HookResult::Modify(new_payload) => {
                    current_payload = new_payload;
                }
                HookResult::Cancel { reason } => {
                    info!(
                        "Hook {:?} cancelled by plugin '{}': {}",
                        hook_type, handler.plugin_id, reason
                    );
                    return HookExecutionResult::Cancelled { reason };
                }
                HookResult::Error { message } => {
                    error!(
                        "Hook {:?} error in plugin '{}': {}",
                        hook_type, handler.plugin_id, message
                    );
                    return HookExecutionResult::Error { message };
                }
            }
        }

        HookExecutionResult::Continue(current_payload)
    }

    /// Check if any handlers are registered for a hook type
    pub async fn has_handlers(&self, hook_type: HookType) -> bool {
        let handlers = self.handlers.read().await;
        handlers
            .get(&hook_type)
            .map(|h| !h.is_empty())
            .unwrap_or(false)
    }

    /// List all registered hooks
    pub async fn list_hooks(&self) -> Vec<(HookType, Vec<String>)> {
        let handlers = self.handlers.read().await;
        handlers
            .iter()
            .map(|(hook_type, handlers_list)| {
                let plugin_ids: Vec<String> =
                    handlers_list.iter().map(|h| h.plugin_id.clone()).collect();
                (*hook_type, plugin_ids)
            })
            .collect()
    }
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of hook execution
#[derive(Debug, Clone)]
pub enum HookExecutionResult {
    /// Continue with (possibly modified) payload
    Continue(HookPayload),
    /// Operation was cancelled
    Cancelled { reason: String },
    /// Error occurred
    Error { message: String },
}

impl HookExecutionResult {
    /// Check if execution should continue
    pub fn should_continue(&self) -> bool {
        matches!(self, HookExecutionResult::Continue(_))
    }

    /// Get the payload if continuing
    pub fn payload(self) -> Option<HookPayload> {
        match self {
            HookExecutionResult::Continue(payload) => Some(payload),
            _ => None,
        }
    }
}

/// Helper macros for hook execution
#[macro_export]
macro_rules! execute_hooks {
    ($registry:expr, $hook_type:expr, $payload:expr) => {{
        use $crate::plugins::hooks::HookExecutionResult;

        match $registry.execute($hook_type, $payload).await {
            HookExecutionResult::Continue(payload) => Ok(payload),
            HookExecutionResult::Cancelled { reason } => {
                Err($crate::error::SyscityError::Validation(reason))
            }
            HookExecutionResult::Error { message } => {
                Err($crate::error::SyscityError::Internal(message))
            }
        }
    }};
}

/// Convenience builder for hook handlers
pub struct HookHandlerBuilder {
    plugin_id: String,
    hook_type: HookType,
    priority: i32,
}

impl HookHandlerBuilder {
    /// Create a new builder
    pub fn new(plugin_id: impl Into<String>, hook_type: HookType) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            hook_type,
            priority: 100, // Default priority
        }
    }

    /// Set priority (lower = earlier)
    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Build with a sync handler
    pub fn handler<F>(self, f: F) -> HookHandler
    where
        F: Fn(HookPayload) -> HookResult + Send + Sync + 'static,
    {
        let plugin_id = self.plugin_id;
        let hook_type = self.hook_type;
        let priority = self.priority;

        HookHandler {
            plugin_id,
            hook_type,
            priority,
            handler: Arc::new(move |payload| {
                let result = f(payload);
                Box::pin(async move { result })
            }),
        }
    }

    /// Build with an async handler
    pub fn async_handler<F, Fut>(self, f: F) -> HookHandler
    where
        F: Fn(HookPayload) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = HookResult> + Send + 'static,
    {
        let plugin_id = self.plugin_id;
        let hook_type = self.hook_type;
        let priority = self.priority;

        HookHandler {
            plugin_id,
            hook_type,
            priority,
            handler: Arc::new(move |payload| Box::pin(f(payload))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_type_display() {
        assert_eq!(HookType::BeforeMessageProcess.to_string(), "before_message_process");
        assert_eq!(HookType::AfterMessageProcess.to_string(), "after_message_process");
        assert_eq!(HookType::BeforeToolExecute.to_string(), "before_tool_execute");
        assert_eq!(HookType::SessionStart.to_string(), "session_start");
        assert_eq!(HookType::ConfigLoad.to_string(), "config_load");
    }

    #[test]
    fn test_hook_type_serde_roundtrip() {
        let types = vec![
            HookType::BeforeMessageProcess,
            HookType::AfterMessageProcess,
            HookType::BeforeToolExecute,
            HookType::AfterToolExecute,
            HookType::SessionStart,
            HookType::SessionEnd,
            HookType::ConfigLoad,
            HookType::BeforeProviderCall,
            HookType::AfterProviderCall,
        ];
        for ht in types {
            let json = serde_json::to_value(&ht).unwrap();
            let decoded: HookType = serde_json::from_value(json).unwrap();
            assert_eq!(decoded, ht);
        }
    }

    #[tokio::test]
    async fn test_hook_registry_empty_execute() {
        let registry = HookRegistry::new();
        let payload = HookPayload::Session {
            session_id: "s1".to_string(),
            user_id: "u1".to_string(),
            agent_id: None,
        };
        let result = registry
            .execute(HookType::SessionStart, payload.clone())
            .await;
        assert!(result.should_continue());
        assert!(matches!(result.payload(), Some(HookPayload::Session { .. })));
    }

    #[tokio::test]
    async fn test_hook_registry_register_and_execute() {
        let registry = HookRegistry::new();
        let handler = HookHandlerBuilder::new("plugin-1", HookType::BeforeToolExecute)
            .priority(10)
            .handler(|_payload| HookResult::Continue);
        registry.register(handler).await;

        assert!(registry.has_handlers(HookType::BeforeToolExecute).await);
        assert!(!registry.has_handlers(HookType::AfterToolExecute).await);

        let payload = HookPayload::ToolExecute {
            tool_name: "echo".to_string(),
            parameters: serde_json::json!({}),
            result: None,
        };
        let result = registry.execute(HookType::BeforeToolExecute, payload).await;
        assert!(result.should_continue());
    }

    #[tokio::test]
    async fn test_hook_registry_modify_payload() {
        let registry = HookRegistry::new();
        let handler =
            HookHandlerBuilder::new("plugin-1", HookType::BeforeToolExecute).handler(|_payload| {
                HookResult::Modify(HookPayload::ToolExecute {
                    tool_name: "modified".to_string(),
                    parameters: serde_json::json!({}),
                    result: None,
                })
            });
        registry.register(handler).await;

        let payload = HookPayload::ToolExecute {
            tool_name: "original".to_string(),
            parameters: serde_json::json!({}),
            result: None,
        };
        let result = registry.execute(HookType::BeforeToolExecute, payload).await;
        assert!(result.should_continue());
        if let Some(HookPayload::ToolExecute { tool_name, .. }) = result.payload() {
            assert_eq!(tool_name, "modified");
        } else {
            panic!("Expected ToolExecute payload");
        }
    }

    #[tokio::test]
    async fn test_hook_registry_cancel() {
        let registry = HookRegistry::new();
        let handler = HookHandlerBuilder::new("plugin-1", HookType::BeforeToolExecute)
            .handler(|_payload| HookResult::Cancel { reason: "blocked".to_string() });
        registry.register(handler).await;

        let payload = HookPayload::ToolExecute {
            tool_name: "rm".to_string(),
            parameters: serde_json::json!({}),
            result: None,
        };
        let result = registry.execute(HookType::BeforeToolExecute, payload).await;
        assert!(!result.should_continue());
        assert!(matches!(result, HookExecutionResult::Cancelled { reason } if reason == "blocked"));
    }

    #[tokio::test]
    async fn test_hook_registry_error() {
        let registry = HookRegistry::new();
        let handler =
            HookHandlerBuilder::new("plugin-1", HookType::BeforeToolExecute).handler(|_payload| {
                HookResult::Error {
                    message: "something went wrong".to_string(),
                }
            });
        registry.register(handler).await;

        let payload = HookPayload::ToolExecute {
            tool_name: "echo".to_string(),
            parameters: serde_json::json!({}),
            result: None,
        };
        let result = registry.execute(HookType::BeforeToolExecute, payload).await;
        assert!(!result.should_continue());
        assert!(
            matches!(result, HookExecutionResult::Error { message } if message == "something went wrong")
        );
    }

    #[tokio::test]
    async fn test_hook_registry_priority_ordering() {
        let registry = HookRegistry::new();
        let handler1 = HookHandlerBuilder::new("plugin-1", HookType::ConfigLoad)
            .priority(20)
            .handler(|_payload| HookResult::Continue);
        let handler2 = HookHandlerBuilder::new("plugin-2", HookType::ConfigLoad)
            .priority(10)
            .handler(|_payload| HookResult::Continue);
        registry.register(handler1).await;
        registry.register(handler2).await;

        let hooks = registry.list_hooks().await;
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].1, vec!["plugin-2", "plugin-1"]);
    }

    #[tokio::test]
    async fn test_hook_registry_unregister_plugin() {
        let registry = HookRegistry::new();
        let handler = HookHandlerBuilder::new("plugin-1", HookType::SessionStart)
            .handler(|_payload| HookResult::Continue);
        registry.register(handler).await;
        assert!(registry.has_handlers(HookType::SessionStart).await);

        registry.unregister_plugin("plugin-1").await;
        assert!(!registry.has_handlers(HookType::SessionStart).await);
    }

    #[tokio::test]
    async fn test_hook_registry_list_hooks() {
        let registry = HookRegistry::new();
        let handler1 = HookHandlerBuilder::new("plugin-1", HookType::BeforeMessageProcess)
            .handler(|_payload| HookResult::Continue);
        let handler2 = HookHandlerBuilder::new("plugin-2", HookType::BeforeMessageProcess)
            .handler(|_payload| HookResult::Continue);
        registry.register(handler1).await;
        registry.register(handler2).await;

        let hooks = registry.list_hooks().await;
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0].0, HookType::BeforeMessageProcess);
        assert_eq!(hooks[0].1.len(), 2);
    }

    #[tokio::test]
    async fn test_hook_handler_builder_async() {
        let handler = HookHandlerBuilder::new("plugin-1", HookType::AfterProviderCall)
            .priority(5)
            .async_handler(|_payload| async move { HookResult::Continue });
        assert_eq!(handler.plugin_id, "plugin-1");
        assert_eq!(handler.hook_type, HookType::AfterProviderCall);
        assert_eq!(handler.priority, 5);
    }

    #[tokio::test]
    async fn test_hook_payload_serde_roundtrip() {
        let payload = HookPayload::MessageProcess {
            session_id: "s1".to_string(),
            user_id: "u1".to_string(),
            content: "hello".to_string(),
            channel: "telegram".to_string(),
        };
        let json = serde_json::to_value(&payload).unwrap();
        let decoded: HookPayload = serde_json::from_value(json).unwrap();
        assert!(
            matches!(decoded, HookPayload::MessageProcess { session_id, .. } if session_id == "s1")
        );
    }
}
