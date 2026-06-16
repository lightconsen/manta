//! Core [`Capability`] trait and [`CapabilityResult`].
//!
//! A `Capability` is anything an Agent can discover and invoke — a
//! logical tool (file read, shell), a desktop action (click, type,
//! screenshot), or a physical-device operation (motor move, camera
//! capture).

use serde_json::Value;
use tokio::sync::broadcast;

use crate::device::safety::SafetyRule;

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

/// An event emitted by an observable device capability (e.g. a sensor
/// reading or a motor position update).
///
/// Events flow through a [`tokio::sync::broadcast`] channel so multiple
/// consumers (TUI, WebSocket, Agent) can subscribe independently.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceEvent {
    /// Device identifier, e.g. `"sensor-01"`.
    pub device_id: String,
    /// Capability name, e.g. `"sensor.read_temperature"`.
    pub capability: String,
    /// When the event was generated (UNIX epoch millis).
    pub timestamp_millis: u64,
    /// Event payload.
    pub data: serde_json::Value,
}

/// A [`Capability`] that can emit a stream of events.
///
/// This is separate from `Capability` because most capabilities are
/// request-response (execute once, get a result).  Streaming capabilities
/// (continuous sensors, live video feeds, etc.) implement this trait so
/// consumers can subscribe to event streams without polling.
#[async_trait::async_trait]
pub trait ObservableCapability: Send + Sync {
    /// Subscribe to the event stream for this capability.
    ///
    /// Returns a `broadcast::Receiver` that yields [`DeviceEvent`]s.
    /// The channel capacity is determined by the implementation.
    fn subscribe(&self) -> broadcast::Receiver<DeviceEvent>;
}

/// Unified interface for any action an Agent can take.
#[async_trait::async_trait]
pub trait Capability: Send + Sync {
    /// Stable identifier, e.g. `"shell"`, `"click"`, `"motor.move_to"`.
    fn name(&self) -> &str;

    /// JSON Schema describing the `params` argument of [`execute`](Self::execute).
    fn param_schema(&self) -> Value;

    /// Execute this capability with the given parameters.
    async fn execute(&self, params: Value) -> CapabilityResult;

    /// Optional safety constraints enforced before execution.
    fn safety_rules(&self) -> Vec<SafetyRule> {
        vec![]
    }

    /// Upcast to [`ObservableCapability`] if this capability supports
    /// streaming events.
    ///
    /// Returns `None` by default. Override to return `Some(self)`.
    fn as_observable(&self) -> Option<&dyn ObservableCapability> {
        None
    }
}
