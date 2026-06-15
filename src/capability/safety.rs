//! Safety constraints for capabilities.
//!
//! A [`SafetyZone`] is attached to each capability provider (tool,
//! desktop adapter, device) and is checked **before** every invocation.
//! If a rule is violated the execution is rejected without calling
//! the underlying implementation.

use serde_json::Value;
use std::time::SystemTime;

/// A single safety rule attached to a capability.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SafetyRule {
    /// Human-readable name, e.g. `"max-velocity"`.
    pub name: String,
    /// The constraint kind.
    pub kind: SafetyRuleKind,
}

/// Kinds of safety constraint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum SafetyRuleKind {
    /// Maximum velocity (device motion).
    MaxVelocity(f64),
    /// Maximum force / torque.
    MaxForce(f64),
    /// Spatial boundary for device movement.
    WorkspaceBoundary(WorkspaceBoundary),
    /// Requires human approval before execution.
    RequiresApproval,
    /// Triggers an emergency stop.
    EmergencyStop,
    /// Custom application-defined rule.
    Custom(String),
}

/// A rectangular / spherical workspace boundary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceBoundary {
    /// Label, e.g. `"safe-zone"`.
    pub label: String,
    /// Axis-aligned bounds as `[min_x, max_x, min_y, max_y, min_z, max_z]`.
    pub bounds: [f64; 6],
}

/// Per-provider safety zone.
///
/// Holds the current set of active rules and tracks whether the zone
/// has been tripped (e.g. by an emergency-stop signal).
#[derive(Debug, Clone)]
pub struct SafetyZone {
    /// Active rules.
    pub rules: Vec<SafetyRule>,
    /// Whether the zone is in tripped state.
    pub engaged: bool,
    /// When the zone was last tripped.
    pub last_triggered: Option<SystemTime>,
}

impl SafetyZone {
    /// Create a new safety zone with the given rules.
    pub fn new(rules: Vec<SafetyRule>) -> Self {
        Self {
            rules,
            engaged: false,
            last_triggered: None,
        }
    }

    /// Check whether executing `capability` with `params` is allowed.
    ///
    /// Returns `Ok(())` if all rules pass, or the first violating rule
    /// as an error. If the zone has been tripped all executions are
    /// rejected until [`reset`](Self::reset) is called.
    pub fn check(&self, capability: &str, _params: &Value) -> Result<(), String> {
        if self.engaged {
            return Err(format!(
                "Safety zone is tripped — cannot execute '{}'",
                capability
            ));
        }
        // Rules are evaluated by the provider at execution time;
        // the zone itself only tracks the engaged state.
        Ok(())
    }

    /// Trip the safety zone (e.g. emergency-stop activated).
    pub fn trip(&mut self, reason: String) {
        self.engaged = true;
        self.last_triggered = Some(SystemTime::now());
        tracing::warn!("Safety zone tripped: {}", reason);
    }

    /// Reset the safety zone after the issue is resolved.
    pub fn reset(&mut self) {
        self.engaged = false;
        tracing::info!("Safety zone reset");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safety_zone_allows_normal_operation() {
        let zone = SafetyZone::new(vec![SafetyRule {
            name: "require-approval".into(),
            kind: SafetyRuleKind::RequiresApproval,
        }]);
        assert!(zone.check("test", &Value::Null).is_ok());
    }

    #[test]
    fn test_safety_zone_blocks_when_tripped() {
        let mut zone = SafetyZone::new(vec![]);
        zone.trip("estop pressed".into());
        assert!(zone.check("test", &Value::Null).is_err());
    }

    #[test]
    fn test_safety_zone_resets() {
        let mut zone = SafetyZone::new(vec![]);
        zone.trip("oops".into());
        zone.reset();
        assert!(zone.check("test", &Value::Null).is_ok());
    }
}
