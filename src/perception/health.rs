//! Per-source health tracking for the [`PerceptionRegistry`].
//!
//! Tracks success/failure history for each source and applies an adaptive
//! backoff strategy: a source that times out repeatedly is moved through
//! `Healthy → Degraded → Quarantined`, has its poll timeout increased, and
//! its poll interval lengthened (exponential backoff capped at 5 minutes).
//!
//! # Lifecycle
//!
//! ```text
//! Healthy ──(3 consecutive failures)──> Degraded
//! Degraded ──(success)──> Healthy
//! Degraded ──(5 more failures)──> Quarantined
//! Quarantined ──(periodic probe success)──> Healthy
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Health state of a single perception source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum HealthState {
    /// Source is responding normally.
    #[default]
    Healthy,
    /// Source has had recent failures but is still being polled.
    Degraded,
    /// Source has been disabled from the active poll cycle until it recovers.
    Quarantined,
}

/// Health metrics and adaptive parameters for one source.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceHealth {
    /// Current state.
    pub state: HealthState,
    /// Total successful polls.
    pub success_count: u64,
    /// Total failed polls (timeout or error).
    pub failure_count: u64,
    /// Timestamp of the most recent successful poll.
    #[serde(skip)]
    pub last_success: Option<Instant>,
    /// Timestamp of the most recent failed poll.
    #[serde(skip)]
    pub last_failure: Option<Instant>,
    /// Most recent error message, if any.
    pub last_error: Option<String>,
    /// Current poll timeout (adaptively increased on repeated failures).
    pub current_timeout_ms: u64,
    /// Current poll interval (adaptively increased while degraded/quarantined).
    pub current_interval_ms: u64,
    /// Consecutive failure streak (resets on success).
    pub consecutive_failures: u64,
}

impl SourceHealth {
    fn new(default_timeout: Duration, default_interval: Duration) -> Self {
        Self {
            state: HealthState::Healthy,
            success_count: 0,
            failure_count: 0,
            last_success: None,
            last_failure: None,
            last_error: None,
            current_timeout_ms: default_timeout.as_millis() as u64,
            current_interval_ms: default_interval.as_millis() as u64,
            consecutive_failures: 0,
        }
    }

    /// Convenience: return current timeout as a Duration.
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.current_timeout_ms)
    }

    /// Convenience: return current interval as a Duration.
    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.current_interval_ms)
    }
}

/// Tunables for the health tracker.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// Default per-source poll timeout when healthy.
    pub default_timeout: Duration,
    /// Default per-source poll interval when healthy.
    pub default_interval: Duration,
    /// Max poll timeout (cap when growing).
    pub max_timeout: Duration,
    /// Max poll interval (cap when growing).
    pub max_interval: Duration,
    /// Consecutive failures before transitioning Healthy → Degraded.
    pub degrade_threshold: u64,
    /// Total failures (since last success) before Degraded → Quarantined.
    pub quarantine_threshold: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(2),
            default_interval: Duration::from_secs(5),
            max_timeout: Duration::from_secs(10),
            max_interval: Duration::from_secs(300),
            degrade_threshold: 3,
            quarantine_threshold: 8,
        }
    }
}

/// Tracks health for all registered perception sources.
#[derive(Debug)]
pub struct HealthTracker {
    config: HealthConfig,
    sources: HashMap<String, SourceHealth>,
}

impl HealthTracker {
    /// Create a new tracker with the given config.
    pub fn new(config: HealthConfig) -> Self {
        Self {
            config,
            sources: HashMap::new(),
        }
    }

    /// Ensure a source has a tracking entry; called on registration.
    pub fn touch(&mut self, name: &str) {
        self.sources.entry(name.to_string()).or_insert_with(|| {
            SourceHealth::new(self.config.default_timeout, self.config.default_interval)
        });
    }

    /// Remove tracking for a deregistered source.
    pub fn forget(&mut self, name: &str) {
        self.sources.remove(name);
    }

    /// Snapshot the current map of source name → health.
    pub fn snapshot(&self) -> HashMap<String, SourceHealth> {
        self.sources.clone()
    }

    /// Lookup a source's current timeout (used before launching a poll).
    pub fn timeout_for(&self, name: &str) -> Duration {
        self.sources
            .get(name)
            .map(|h| h.timeout())
            .unwrap_or(self.config.default_timeout)
    }

    /// Lookup a source's current interval (used by per-source tickers).
    pub fn interval_for(&self, name: &str) -> Duration {
        self.sources
            .get(name)
            .map(|h| h.interval())
            .unwrap_or(self.config.default_interval)
    }

    /// True if a source is quarantined and should be skipped this cycle.
    pub fn is_quarantined(&self, name: &str) -> bool {
        self.sources
            .get(name)
            .map(|h| h.state == HealthState::Quarantined)
            .unwrap_or(false)
    }

    /// Record a successful poll. Resets failure streak and timeouts toward
    /// defaults.
    ///
    /// Returns the new `HealthState` iff this call **changed** the state
    /// (typically `Degraded`/`Quarantined` → `Healthy` after recovery).
    /// Callers can use this to emit recovery anomalies on the derived
    /// event hub.
    pub fn record_success(&mut self, name: &str) -> Option<HealthState> {
        let cfg = self.config.clone();
        let entry = self
            .sources
            .entry(name.to_string())
            .or_insert_with(|| SourceHealth::new(cfg.default_timeout, cfg.default_interval));

        let prev = entry.state;
        entry.success_count = entry.success_count.saturating_add(1);
        entry.last_success = Some(Instant::now());
        entry.last_error = None;
        entry.consecutive_failures = 0;

        // Recover toward defaults.
        entry.state = HealthState::Healthy;
        entry.current_timeout_ms = cfg.default_timeout.as_millis() as u64;
        entry.current_interval_ms = cfg.default_interval.as_millis() as u64;

        if prev != entry.state {
            Some(entry.state)
        } else {
            None
        }
    }

    /// Record a poll timeout (no response within current_timeout).
    /// Returns the new state iff the failure caused a transition.
    pub fn record_timeout(&mut self, name: &str) -> Option<HealthState> {
        self.record_failure_inner(name, "poll timeout".to_string())
    }

    /// Record a generic error from observe().
    /// Returns the new state iff the failure caused a transition.
    pub fn record_error(&mut self, name: &str, err: String) -> Option<HealthState> {
        self.record_failure_inner(name, err)
    }

    fn record_failure_inner(&mut self, name: &str, msg: String) -> Option<HealthState> {
        let cfg = self.config.clone();
        let entry = self
            .sources
            .entry(name.to_string())
            .or_insert_with(|| SourceHealth::new(cfg.default_timeout, cfg.default_interval));

        let prev = entry.state;
        entry.failure_count = entry.failure_count.saturating_add(1);
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        entry.last_failure = Some(Instant::now());
        entry.last_error = Some(msg);

        // Adaptive backoff: double timeout/interval on each failure, capped.
        let new_timeout =
            (entry.current_timeout_ms.saturating_mul(2)).min(cfg.max_timeout.as_millis() as u64);
        let new_interval =
            (entry.current_interval_ms.saturating_mul(2)).min(cfg.max_interval.as_millis() as u64);
        entry.current_timeout_ms = new_timeout;
        entry.current_interval_ms = new_interval;

        // State transition.
        entry.state = if entry.consecutive_failures >= cfg.quarantine_threshold {
            HealthState::Quarantined
        } else if entry.consecutive_failures >= cfg.degrade_threshold {
            HealthState::Degraded
        } else {
            entry.state
        };

        if prev != entry.state {
            Some(entry.state)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast_cfg() -> HealthConfig {
        HealthConfig {
            default_timeout: Duration::from_millis(100),
            default_interval: Duration::from_millis(100),
            max_timeout: Duration::from_millis(800),
            max_interval: Duration::from_millis(800),
            degrade_threshold: 2,
            quarantine_threshold: 4,
        }
    }

    #[test]
    fn test_record_success_resets_state() {
        let mut h = HealthTracker::new(fast_cfg());
        h.record_error("a", "boom".into());
        h.record_error("a", "boom".into());
        assert_eq!(h.snapshot()["a"].state, HealthState::Degraded);
        h.record_success("a");
        let snap = h.snapshot();
        assert_eq!(snap["a"].state, HealthState::Healthy);
        assert_eq!(snap["a"].consecutive_failures, 0);
    }

    #[test]
    fn test_degrade_and_quarantine_thresholds() {
        let mut h = HealthTracker::new(fast_cfg());
        h.record_timeout("s");
        assert_eq!(h.snapshot()["s"].state, HealthState::Healthy);
        h.record_timeout("s");
        assert_eq!(h.snapshot()["s"].state, HealthState::Degraded);
        h.record_timeout("s");
        h.record_timeout("s");
        assert_eq!(h.snapshot()["s"].state, HealthState::Quarantined);
    }

    #[test]
    fn test_backoff_caps_at_max() {
        let mut h = HealthTracker::new(fast_cfg());
        for _ in 0..20 {
            h.record_timeout("s");
        }
        let snap = h.snapshot();
        assert_eq!(snap["s"].current_timeout_ms, 800);
        assert_eq!(snap["s"].current_interval_ms, 800);
    }

    #[test]
    fn test_forget_removes_entry() {
        let mut h = HealthTracker::new(fast_cfg());
        h.touch("a");
        assert!(h.snapshot().contains_key("a"));
        h.forget("a");
        assert!(!h.snapshot().contains_key("a"));
    }

    #[test]
    fn test_is_quarantined_default_false() {
        let h = HealthTracker::new(fast_cfg());
        assert!(!h.is_quarantined("never_seen"));
    }
}
