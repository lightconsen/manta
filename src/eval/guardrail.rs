//! Guardrail layer for the scalar optimizer (§十二 护栏).
//!
//! Phase 4 inserts safety between candidate generation and application:
//!
//! - **Cost cap** — a candidate is rejected when the global [`CostGuard`] has
//!   tripped its budget.
//! - **Shadow eval** — a candidate is evaluated by a [`ShadowEvaluator`] before
//!   it is applied; failures are rejected and recorded as `gate_fail` traces.
//! - **Circuit breaker** — consecutive gate failures / rollbacks open the
//!   breaker, which pauses the scheduler (`OptimizerRuntime::paused`) until
//!   `eval.optimizer.resume` clears it (or the cooldown elapses).
//! - **Auto-rollback** — [`ScalarOptimizer::rollback`] writes a previously
//!   applied scalar back via the same CAS machinery and records a `rollback`
//!   trace (see `optimizer.rs`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::RwLock;

use crate::eval::optimizer::now_ms;
use crate::eval::pending_badcase::{PendingBadcaseStore, PendingSource, PendingStatus};
use crate::eval::ScalarCandidate;
use crate::gateway::feedback::FeedbackVoteKind;
use crate::gateway::FeedbackStore;

/// Immutable snapshot of the circuit breaker surfaced by `eval.optimizer.status`.
#[derive(Debug, Clone, Serialize)]
pub struct BreakerSnapshot {
    pub failures: u32,
    pub tripped: bool,
}

/// Circuit breaker for automatic optimizer pauses.
///
/// Closed → **open** when the accumulated failure count reaches `threshold`.
/// Once open, `run()` reports `circuit_open` and the scheduler skips ticks
/// until `reset()` (via `eval.optimizer.resume`) or the cooldown elapses.
#[derive(Debug, Default)]
pub struct CircuitBreaker {
    state: RwLock<BreakerState>,
}

#[derive(Debug, Clone, Default)]
struct BreakerState {
    failures: u32,
    tripped: bool,
    tripped_at: Option<i64>,
}

impl CircuitBreaker {
    /// Create an empty (closed) breaker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a gate failure or rollback. Returns `true` when the breaker
    /// just transitioned to the open state.
    pub async fn record_failure(&self, threshold: u32) -> bool {
        let mut st = self.state.write().await;
        st.failures = st.failures.saturating_add(1);
        if st.failures >= threshold.max(1) && !st.tripped {
            st.tripped = true;
            st.tripped_at = Some(now_ms());
            return true;
        }
        false
    }

    /// Record a successful apply. In a half-open window (cooldown elapsed but
    /// still tripped) a success closes the breaker.
    pub async fn record_success(&self) {
        let mut st = self.state.write().await;
        st.failures = 0;
        st.tripped = false;
        st.tripped_at = None;
    }

    /// Clear the breaker (the `eval.optimizer.resume` escape hatch).
    pub async fn reset(&self) {
        let mut st = self.state.write().await;
        st.failures = 0;
        st.tripped = false;
        st.tripped_at = None;
    }

    /// Whether the breaker is currently open — tripped and still inside the
    /// cooldown window.
    pub async fn is_open(&self, cooldown: Duration) -> bool {
        let st = self.state.read().await;
        if !st.tripped {
            return false;
        }
        match st.tripped_at {
            Some(t) => now_ms().saturating_sub(t) < cooldown.as_millis() as i64,
            None => false,
        }
    }

    pub async fn snapshot(&self) -> BreakerSnapshot {
        let st = self.state.read().await;
        BreakerSnapshot {
            failures: st.failures,
            tripped: st.tripped,
        }
    }
}

/// Evaluates a scalar candidate before it is applied (§十二 护栏 · shadow eval).
///
/// `Ok(true)` passes the gate; `Ok(false)` rejects the candidate. The default
/// production implementation aggregates online signals (Like/Dislike ratio and
/// pending `online:risk` badcases); tests inject deterministic stubs.
#[async_trait]
pub trait ShadowEvaluator: Send + Sync {
    async fn evaluate(&self, candidate: &ScalarCandidate) -> Result<bool, String>;

    /// Post-apply online-signal anomaly check (§十二 护栏 · 自动回滚). Returns
    /// `true` when the online signals indicate the just-applied change
    /// regressed and should be rolled back. Defaults to `false` (no anomaly)
    /// for evaluators without online-signal state; the
    /// [`OnlineSignalShadowEvaluator`] overrides it with the real signal
    /// aggregation.
    async fn is_anomalous(&self) -> Result<bool, String> {
        Ok(false)
    }
}

/// Shadow evaluator over online signals.
///
/// A candidate is rejected when (a) the recent Like/Dislike ratio shows a
/// majority of down-votes, or (b) the pending `online:risk` badcase pool has
/// grown beyond `max_online_risks`. When neither signal store is wired the
/// gate passes (no data → no veto).
#[derive(Debug, Clone)]
pub struct OnlineSignalShadowEvaluator {
    feedback: Option<Arc<FeedbackStore>>,
    badcases: Option<Arc<PendingBadcaseStore>>,
    max_down_ratio: f64,
    min_votes: u32,
    max_online_risks: u32,
    window_ms: i64,
}

impl OnlineSignalShadowEvaluator {
    pub fn new(
        feedback: Option<Arc<FeedbackStore>>,
        badcases: Option<Arc<PendingBadcaseStore>>,
        max_down_ratio: f64,
        min_votes: u32,
        max_online_risks: u32,
        window_ms: i64,
    ) -> Self {
        Self {
            feedback,
            badcases,
            max_down_ratio,
            min_votes,
            max_online_risks,
            window_ms,
        }
    }

    async fn down_vote_ratio_rejected(&self) -> Result<bool, String> {
        let Some(store) = &self.feedback else {
            return Ok(false);
        };
        let since = now_ms().saturating_sub(self.window_ms);
        let ups = store
            .list_votes_by(FeedbackVoteKind::Up, since, 500)
            .await
            .map_err(|e| format!("failed to read up-votes: {e}"))?;
        let downs = store
            .list_votes_by(FeedbackVoteKind::Down, since, 500)
            .await
            .map_err(|e| format!("failed to read down-votes: {e}"))?;
        let total = ups.len() + downs.len();
        if total < self.min_votes as usize {
            return Ok(false);
        }
        let ratio = downs.len() as f64 / total as f64;
        Ok(ratio > self.max_down_ratio)
    }

    async fn online_risk_rejected(&self) -> Result<bool, String> {
        let Some(store) = &self.badcases else {
            return Ok(false);
        };
        let since = now_ms().saturating_sub(self.window_ms);
        let pending = store
            .list_pending(PendingStatus::Pending, 500)
            .await
            .map_err(|e| format!("failed to read pending badcases: {e}"))?;
        let recent_risks = pending
            .iter()
            .filter(|b| b.source == PendingSource::OnlineRisk && b.created_at >= since)
            .count();
        Ok(recent_risks >= self.max_online_risks as usize)
    }

    /// Online-signal anomaly check independent of a specific candidate. Used
    /// post-apply to decide whether a change regressed (§十二 护栏 · 自动回滚).
    pub async fn is_anomalous(&self) -> Result<bool, String> {
        Ok(self.down_vote_ratio_rejected().await? || self.online_risk_rejected().await?)
    }
}

#[async_trait]
impl ShadowEvaluator for OnlineSignalShadowEvaluator {
    async fn evaluate(&self, _candidate: &ScalarCandidate) -> Result<bool, String> {
        Ok(!self.is_anomalous().await?)
    }

    async fn is_anomalous(&self) -> Result<bool, String> {
        OnlineSignalShadowEvaluator::is_anomalous(self).await
    }
}

/// No-op evaluator: always passes. Used when guardrails are enabled but no
/// signal store is available (the gate is then only the cost cap).
#[derive(Debug, Default)]
pub struct NoopShadowEvaluator;

#[async_trait]
impl ShadowEvaluator for NoopShadowEvaluator {
    async fn evaluate(&self, _candidate: &ScalarCandidate) -> Result<bool, String> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn breaker_trips_at_threshold_and_success_closes() {
        let breaker = CircuitBreaker::new();
        assert!(!breaker.is_open(Duration::from_secs(60)).await);

        assert!(!breaker.record_failure(2).await, "first failure below threshold");
        assert!(breaker.record_failure(2).await, "second failure trips the breaker");
        assert!(breaker.is_open(Duration::from_secs(60)).await);
        let snap = breaker.snapshot().await;
        assert_eq!(snap.failures, 2);
        assert!(snap.tripped);

        // A success (post-cooldown probe) closes the breaker.
        breaker.record_success().await;
        assert!(!breaker.is_open(Duration::from_secs(60)).await);
        let snap = breaker.snapshot().await;
        assert_eq!(snap.failures, 0);
        assert!(!snap.tripped);
    }

    #[tokio::test]
    async fn breaker_reset_clears_trip() {
        let breaker = CircuitBreaker::new();
        breaker.record_failure(1).await;
        assert!(breaker.is_open(Duration::from_secs(60)).await);
        breaker.reset().await;
        assert!(!breaker.is_open(Duration::from_secs(60)).await);
    }

    #[tokio::test]
    async fn noop_evaluator_passes() {
        let cand = ScalarCandidate {
            path: "default_agent.temperature",
            current: 0.7,
            proposed: 0.8,
            reason: "probe".to_string(),
        };
        assert!(NoopShadowEvaluator.evaluate(&cand).await.unwrap());
    }

    #[tokio::test]
    async fn shadow_evaluator_is_anomalous_defaults_false() {
        // The no-op evaluator never reports a post-apply anomaly.
        assert!(!NoopShadowEvaluator.is_anomalous().await.unwrap());
    }

    #[tokio::test]
    async fn online_evaluator_is_anomalous_false_without_signal_stores() {
        let ev = OnlineSignalShadowEvaluator::new(None, None, 0.5, 10, 3, 24 * 3600 * 1000);
        assert!(!ev.is_anomalous().await.unwrap());
        // The pre-apply gate therefore passes too.
        let cand = ScalarCandidate {
            path: "default_agent.temperature",
            current: 0.7,
            proposed: 0.8,
            reason: "probe".to_string(),
        };
        assert!(ev.evaluate(&cand).await.unwrap());
    }
}
