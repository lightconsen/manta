//! Live cost guard for the agent loop.
//!
//! `CostGuard` tracks daily dollar spend (in cents) and hourly action rate,
//! exposing an atomic `budget_exceeded` flag that is checked before every
//! provider call in `Agent::get_completion()`.
//!
//! Counters auto-reset every 24 h (daily) and 1 h (hourly) without requiring
//! a cron job.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Live spending and action-rate tracker.
///
/// All methods are cheaply callable from hot async paths — they use
/// atomics and only lock a `Mutex` for the rare reset check.
pub struct CostGuard {
    /// Accumulated spend today, in cents.
    daily_cents: AtomicU64,
    /// Maximum daily spend in cents (0 = unlimited).
    pub daily_limit_cents: u64,
    /// Actions taken in the current hour.
    hourly_actions: AtomicU64,
    /// Maximum hourly actions (0 = unlimited).
    pub hourly_action_limit: u64,
    /// Set to `true` when any limit is crossed.
    pub budget_exceeded: AtomicBool,
    /// Timestamp of the last daily reset.
    last_daily_reset: Mutex<SystemTime>,
    /// Instant of the last hourly reset.
    last_hourly_reset: Mutex<Instant>,
}

impl std::fmt::Debug for CostGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CostGuard")
            .field("daily_cents", &self.daily_cents.load(Ordering::Relaxed))
            .field("daily_limit_cents", &self.daily_limit_cents)
            .field("hourly_actions", &self.hourly_actions.load(Ordering::Relaxed))
            .field("hourly_action_limit", &self.hourly_action_limit)
            .field("budget_exceeded", &self.budget_exceeded.load(Ordering::Relaxed))
            .finish()
    }
}

impl CostGuard {
    /// Create a new `CostGuard` wrapped in an `Arc`.
    ///
    /// Pass `0` to either limit to disable that guard.
    pub fn new(daily_limit_cents: u64, hourly_action_limit: u64) -> Arc<Self> {
        Arc::new(Self {
            daily_cents: AtomicU64::new(0),
            daily_limit_cents,
            hourly_actions: AtomicU64::new(0),
            hourly_action_limit,
            budget_exceeded: AtomicBool::new(false),
            last_daily_reset: Mutex::new(SystemTime::now()),
            last_hourly_reset: Mutex::new(Instant::now()),
        })
    }

    /// Returns `true` if any spending or rate limit has been exceeded.
    #[inline]
    pub fn is_exceeded(&self) -> bool {
        self.budget_exceeded.load(Ordering::Relaxed)
    }

    /// Record the token usage for one provider call.
    ///
    /// Increments the hourly action counter and accumulates estimated cost.
    /// Trips `budget_exceeded` if a limit is crossed.
    pub fn record_usage(&self, input_tokens: u64, output_tokens: u64, model: &str) {
        self.maybe_reset();

        let (input_cpm, output_cpm) = Self::pricing_for_model(model);
        // cpm = cents per million tokens
        let cost_cents =
            (input_tokens * input_cpm + output_tokens * output_cpm) / 1_000_000;

        let new_daily =
            self.daily_cents.fetch_add(cost_cents, Ordering::Relaxed) + cost_cents;
        let new_hourly = self.hourly_actions.fetch_add(1, Ordering::Relaxed) + 1;

        let daily_exceeded =
            self.daily_limit_cents > 0 && new_daily >= self.daily_limit_cents;
        let hourly_exceeded =
            self.hourly_action_limit > 0 && new_hourly >= self.hourly_action_limit;

        if daily_exceeded || hourly_exceeded {
            self.budget_exceeded.store(true, Ordering::Relaxed);
            tracing::warn!(
                daily_cents = new_daily,
                daily_limit = self.daily_limit_cents,
                hourly_actions = new_hourly,
                hourly_limit = self.hourly_action_limit,
                "CostGuard: budget limit reached"
            );
        }
    }

    /// Current daily spend in cents.
    pub fn daily_spend_cents(&self) -> u64 {
        self.daily_cents.load(Ordering::Relaxed)
    }

    /// Number of actions taken in the current hour.
    pub fn hourly_action_count(&self) -> u64 {
        self.hourly_actions.load(Ordering::Relaxed)
    }

    /// Manually reset the exceeded flag (e.g. after a config change).
    pub fn reset_exceeded(&self) {
        self.budget_exceeded.store(false, Ordering::Relaxed);
    }

    // ── Private ───────────────────────────────────────────────────────────────

    /// Reset daily / hourly counters if the window has elapsed.
    fn maybe_reset(&self) {
        // Daily reset
        if let Ok(mut last) = self.last_daily_reset.lock() {
            if let Ok(elapsed) = last.elapsed() {
                if elapsed >= Duration::from_secs(86_400) {
                    self.daily_cents.store(0, Ordering::Relaxed);
                    // Only clear the flag if no hourly limit is also tripped.
                    if self.hourly_action_limit == 0 {
                        self.budget_exceeded.store(false, Ordering::Relaxed);
                    }
                    *last = SystemTime::now();
                }
            }
        }

        // Hourly reset
        if let Ok(mut last) = self.last_hourly_reset.lock() {
            if last.elapsed() >= Duration::from_secs(3_600) {
                self.hourly_actions.store(0, Ordering::Relaxed);
                // Re-check whether the daily limit is still exceeded.
                let daily_ok = self.daily_limit_cents == 0
                    || self.daily_cents.load(Ordering::Relaxed) < self.daily_limit_cents;
                if daily_ok {
                    self.budget_exceeded.store(false, Ordering::Relaxed);
                }
                *last = Instant::now();
            }
        }
    }

    /// Returns (input_cents_per_million, output_cents_per_million) for the
    /// named model.  Values are approximate and intended for budget guardrails,
    /// not billing accuracy.
    fn pricing_for_model(model: &str) -> (u64, u64) {
        let m = model.to_lowercase();
        if m.contains("opus") {
            (1_500, 7_500)
        } else if m.contains("sonnet") {
            (300, 1_500)
        } else if m.contains("haiku") {
            (25, 125)
        } else if m.contains("gpt-4o") {
            (250, 1_000)
        } else if m.contains("gpt-4") {
            (1_000, 3_000)
        } else if m.contains("gpt-3.5") {
            (50, 150)
        } else {
            // Conservative default
            (300, 1_500)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_guard_no_limits() {
        let guard = CostGuard::new(0, 0);
        guard.record_usage(1_000_000, 500_000, "claude-3-sonnet");
        assert!(!guard.is_exceeded());
    }

    #[test]
    fn test_cost_guard_daily_limit() {
        // Limit of 1 cent
        let guard = CostGuard::new(1, 0);
        // claude-sonnet: 300 cpm input → 1M tokens = 300 cents > 1 cent limit
        guard.record_usage(1_000_000, 0, "claude-3-sonnet");
        assert!(guard.is_exceeded());
    }

    #[test]
    fn test_cost_guard_hourly_limit() {
        let guard = CostGuard::new(0, 2);
        guard.record_usage(100, 100, "claude-3-haiku");
        assert!(!guard.is_exceeded());
        guard.record_usage(100, 100, "claude-3-haiku");
        assert!(guard.is_exceeded());
    }

    #[test]
    fn test_cost_guard_reset_exceeded() {
        let guard = CostGuard::new(0, 1);
        guard.record_usage(100, 100, "claude-3-haiku");
        assert!(guard.is_exceeded());
        guard.reset_exceeded();
        assert!(!guard.is_exceeded());
    }
}
