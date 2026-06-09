//! Configuration for standing orders (persistent background agent programs).

use serde::{Deserialize, Serialize};

/// Top-level standing orders configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandingOrderConfig {
    /// Master switch to enable or disable all standing orders.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// List of standing order definitions.
    #[serde(default)]
    pub orders: Vec<StandingOrderDef>,
}

fn default_enabled() -> bool {
    true
}

impl Default for StandingOrderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            orders: Vec::new(),
        }
    }
}

/// A single standing order definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandingOrderDef {
    /// Unique name / identifier for this order.
    pub name: String,

    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The agent ID to target when this order fires.
    pub agent_id: String,

    /// Cron expression (six-field: sec min hour day-of-month month day-of-week).
    pub schedule: String,

    /// The prompt to send to the agent each time the schedule fires.
    pub prompt: String,

    /// Optional channel name to dispatch the agent's response to.
    /// If `None`, the response is logged but not dispatched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_channel: Option<String>,

    /// Whether this specific order is enabled.
    #[serde(default = "default_order_enabled")]
    pub enabled: bool,

    /// Optional per-order timeout in seconds (default: 120).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

fn default_order_enabled() -> bool {
    true
}
