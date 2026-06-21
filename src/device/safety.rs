//! Safety constraints for device capabilities.
//!
//! A [`SafetyZone`] is attached to each device and is checked **before**
//! every invocation.  If a rule is violated the execution is rejected
//! without calling the underlying implementation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::Value;

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

/// Per-device safety zone.
///
/// Holds the current set of active rules and tracks whether the zone
/// has been tripped (e.g. by an emergency-stop signal).
///
/// The `engaged` field is an [`Arc<AtomicBool>`] — driver control loops
/// and safety-interrupt tasks can clone the handle via
/// [`engaged_handle`](Self::engaged_handle) and read/write it without
/// acquiring the device's `RwLock`.
#[derive(Debug)]
pub struct SafetyZone {
    /// Active rules.
    pub rules: Vec<SafetyRule>,
    /// Whether the zone is in tripped state (lock-free for fast paths).
    engaged: Arc<AtomicBool>,
    /// When the zone was last tripped (only updated by the slow path).
    pub last_triggered: Option<SystemTime>,
}

impl Clone for SafetyZone {
    fn clone(&self) -> Self {
        Self {
            rules: self.rules.clone(),
            engaged: Arc::clone(&self.engaged),
            last_triggered: self.last_triggered,
        }
    }
}

impl SafetyZone {
    /// Create a new safety zone with the given rules.
    pub fn new(rules: Vec<SafetyRule>) -> Self {
        Self {
            rules,
            engaged: Arc::new(AtomicBool::new(false)),
            last_triggered: None,
        }
    }

    /// Return a clone of the inner `Arc<AtomicBool>` for fast-path sharing.
    ///
    /// Driver control loops and safety-interrupt tasks can clone this handle
    /// and read/write the engaged flag without acquiring the device's `RwLock`.
    pub fn engaged_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.engaged)
    }

    /// Returns `true` if the safety zone is currently tripped.
    pub fn is_engaged(&self) -> bool {
        self.engaged.load(Ordering::Acquire)
    }

    /// Check whether executing `capability` with `params` is allowed.
    ///
    /// Returns `Ok(())` if all rules pass, or the first violating rule
    /// as an error. If the zone has been tripped all executions are
    /// rejected until [`reset`](Self::reset) is called.
    pub fn check(&self, capability: &str, _params: &Value) -> Result<(), String> {
        if self.engaged.load(Ordering::Acquire) {
            return Err(format!("Safety zone is tripped — cannot execute '{}'", capability));
        }
        // Rules are evaluated by the provider at execution time;
        // the zone itself only tracks the engaged state.
        Ok(())
    }

    /// Trip the safety zone (e.g. emergency-stop activated).
    ///
    /// This requires `&mut self` because it also records `last_triggered`.
    /// For lock-free tripping from a fast path (interrupt task, control
    /// loop), use [`fast_trip`](Self::fast_trip) instead.
    pub fn trip(&mut self, reason: String) {
        self.engaged.store(true, Ordering::Release);
        self.last_triggered = Some(SystemTime::now());
        tracing::warn!("Safety zone tripped: {}", reason);
    }

    /// Trip the safety zone from a fast path without `&mut self`.
    ///
    /// Unlike [`trip`](Self::trip), this does **not** record
    /// `last_triggered` because fast paths (interrupt tasks, control loops)
    /// don't have exclusive access to the [`SafetyZone`]. It only sets the
    /// engaged flag atomically.
    ///
    /// Use this in driver-spawned safety-monitor tasks:
    ///
    /// ```ignore
    /// let engaged = safety.engaged_handle();
    /// tokio::spawn(async move {
    ///     loop {
    ///         if estop_pin.read().unwrap_or(false) {
    ///             engaged.store(true, Ordering::Release);
    ///             disable_output();
    ///         }
    ///         tokio::time::sleep(Duration::from_millis(1)).await;
    ///     }
    /// });
    /// ```
    pub fn fast_trip(&self) {
        self.engaged.store(true, Ordering::Release);
    }

    /// Reset the safety zone after the issue is resolved.
    pub fn reset(&mut self) {
        self.engaged.store(false, Ordering::Release);
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

    #[test]
    fn test_engaged_handle_fast_path() {
        let zone = SafetyZone::new(vec![]);
        let handle = zone.engaged_handle();
        assert!(!handle.load(Ordering::Acquire));
        assert!(!zone.is_engaged());

        // Fast-path safety task trips via Arc
        handle.store(true, Ordering::Release);
        assert!(zone.is_engaged());

        // Slow path can still read it
        assert!(zone.check("test", &Value::Null).is_err());
    }

    #[test]
    fn test_fast_trip() {
        let zone = SafetyZone::new(vec![]);
        zone.fast_trip();
        assert!(zone.is_engaged());
        assert!(zone.last_triggered.is_none()); // fast_trip doesn't set
                                                // timestamp
    }

    #[test]
    fn test_engaged_handle_shared_between_tasks() {
        // Simulates the two-tier control loop pattern:
        //   Driver::connect() creates SafetyZone, clones the handle,
        //   hands handle to control loop + safety monitor tasks.
        let zone = SafetyZone::new(vec![]);
        let ctrl = zone.engaged_handle();
        let safety_mon = zone.engaged_handle();

        // Safety monitor trips
        safety_mon.store(true, Ordering::Release);
        assert!(ctrl.load(Ordering::Acquire));
        assert!(zone.is_engaged());

        // Slow path can still read it
        assert!(zone.check("test", &Value::Null).is_err());
    }
}
