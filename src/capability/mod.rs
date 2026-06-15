//! Unified Capability abstraction for Syscity.
//!
//! A `Capability` is anything an Agent can discover and invoke — a
//! logical tool (file read, shell), a desktop action (click, type,
//! screenshot), or a physical-device operation (motor move, camera
//! capture).
//!
//! This module provides the common trait and result type. Concrete
//! implementations live in [`providers`] and in [`crate::device`].

use serde_json::Value;

pub mod providers;
pub mod registry;
pub mod safety;

/// Outcome of executing a [`Capability`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CapabilityResult {
    /// Whether execution succeeded.
    pub success: bool,
    /// Structured output data, if any.
    pub output: Option<Value>,
    /// Error message on failure.
    pub error: Option<String>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
}

/// Unified interface for any action an Agent can take.
///
/// # Implementors
///
/// - `src/tools/` tools wrap via [`providers::tool_adapter::ToolCapability`]
/// - `src/computer/` desktop actions wrap via [`providers::computer_adapter`]
/// - `src/device/` device capabilities implement directly
#[async_trait::async_trait]
pub trait Capability: Send + Sync {
    /// Stable identifier, e.g. `"shell"`, `"click"`, `"motor.move_to"`.
    fn name(&self) -> &str;

    /// JSON Schema describing the `params` argument of [`execute`].
    fn param_schema(&self) -> Value;

    /// Execute this capability with the given parameters.
    async fn execute(&self, params: Value) -> CapabilityResult;

    /// Optional safety constraints enforced before execution.
    fn safety_rules(&self) -> Vec<safety::SafetyRule> {
        vec![]
    }
}
