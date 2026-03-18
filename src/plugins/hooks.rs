//! Plugin Hooks System
//!
//! Allows plugins to hook into various events and extend behavior.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

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
        let entry = handlers.entry(handler.hook_type.clone()).or_default();
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
                (hook_type.clone(), plugin_ids)
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
                Err($crate::error::MantaError::Validation(reason))
            }
            HookExecutionResult::Error { message } => {
                Err($crate::error::MantaError::Internal(message))
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
