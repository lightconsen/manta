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
use tracing::{debug, warn};

use crate::agent::{Agent, AgentBuilder, AgentConfig};
use crate::eval::harness::EvalHarness;
use crate::eval::optimizer::now_ms;
use crate::eval::pending_badcase::{PendingBadcaseStore, PendingSource, PendingStatus};
use crate::eval::verdict::{CandidateVerdict, CandidateVerifier, VerdictSubject};
use crate::eval::{compare_versions, ScalarCandidate, TurnSampleStore};
use crate::gateway::feedback::FeedbackVoteKind;
use crate::gateway::shadow_replay::samples_to_replay_turns;
use crate::gateway::FeedbackStore;
use crate::gateway::GatewayState;
use crate::providers::Provider;

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

/// Suite id reported on [`CandidateVerdict`]s produced by the online shadow
/// verifier. There is no YAML suite backing it — the "suite" is the sampled
/// live traffic.
pub const ONLINE_SHADOW_SUITE_ID: &str = "online_shadow";

/// Online-replay candidate verifier (§十二 ⑤⑥ 纪律 · N=1 shadow).
///
/// Instead of a hand-authored regression suite, this verifier replays the most
/// recent sampled production turns ([`TurnSampleStore`]) through a baseline and
/// a candidate agent, then returns the bootstrap comparison of the two
/// per-turn pass/fail sequences via [`compare_replays`]. It plugs into the same
/// [`CandidateVerifier`] gate as [`crate::eval::HarnessCandidateVerifier`], so
/// a regression observed on live traffic rejects the candidate.
///
/// Every failure mode (no sample store, store read error, no recent turns,
/// unknown scalar path, missing provider, harness run error) degrades to
/// `Ok(None)` — no evidence — so a missing LLM / empty sampling pool never
/// fails the optimizer run.
pub struct RealTurnCandidateVerifier {
    /// Sampled production turn store. `None` means "no evidence" (the gate
    /// rejects conservatively, matching `NoopVerifier` semantics).
    pub sample_store: Option<Arc<TurnSampleStore>>,
    /// How many recent samples to replay (newest first).
    pub recent_limit: u32,
    /// Trials per replayed turn in the harness (N=1 is the online shadow form).
    pub trials: usize,
    /// Gateway state used to snapshot config, resolve a provider, and build
    /// the baseline + candidate agents (mirrors `HarnessCandidateVerifier`).
    pub state: Arc<GatewayState>,
    /// Bootstrap iterations for the comparison.
    pub iterations: usize,
    /// Confidence level for the comparison.
    pub confidence: f64,
}

impl RealTurnCandidateVerifier {
    /// Create a verifier over the last `recent_limit` sampled turns, with
    /// `trials` harness trials per turn and the given bootstrap settings.
    /// The constructor shape mirrors [`crate::eval::HarnessCandidateVerifier`].
    pub fn new(
        state: Arc<GatewayState>,
        sample_store: Option<Arc<TurnSampleStore>>,
        recent_limit: u32,
        trials: usize,
        iterations: usize,
        confidence: f64,
    ) -> Self {
        Self {
            sample_store,
            recent_limit,
            trials: trials.max(1),
            iterations: iterations.max(1),
            confidence,
            state,
        }
    }

    /// Build an [`Agent`] the same way `src/gateway/init/agents.rs` does
    /// (mirrors `crate::eval::HarnessCandidateVerifier::build_agent`).
    fn build_agent(
        &self,
        provider: Arc<dyn Provider + Send + Sync>,
        snapshot: &ShadowConfigSnapshot,
        mut cfg: AgentConfig,
    ) -> Result<Arc<Agent>, String> {
        cfg.workspace_dir = snapshot.workspace_dir.clone();
        cfg.workspace_only = snapshot.workspace_only;
        AgentBuilder::new()
            .config(cfg)
            .provider(provider)
            .tools(self.state.tools.registry.clone())
            .model_router(self.state.infra.model_router.clone())
            .model(snapshot.model.clone())
            .planner_model(snapshot.model.clone())
            .skill_manager(self.state.tools.skills_manager.clone())
            .build()
            .map(Arc::new)
            .map_err(|e| e.to_string())
    }
}

#[async_trait]
impl CandidateVerifier for RealTurnCandidateVerifier {
    async fn verify(&self, subject: &VerdictSubject) -> Result<Option<CandidateVerdict>, String> {
        // 1. No sample store → no online evidence.
        let Some(store) = &self.sample_store else {
            warn!("real-turn verifier: no sample store wired; no evidence for {}", subject.path);
            return Ok(None);
        };

        // 2. Read the most recent sampled production turns.
        let samples = match store.list_recent(self.recent_limit).await {
            Ok(s) => s,
            Err(e) => {
                warn!("real-turn verifier: failed to read turn samples: {}", e);
                return Ok(None);
            }
        };
        let turns = samples_to_replay_turns(&samples);
        if turns.is_empty() {
            warn!("real-turn verifier: no recent sampled turns; no evidence for {}", subject.path);
            return Ok(None);
        }

        // 3. Snapshot the config so the config lock is never held across an
        //    `.await` (the harness runs make LLM calls).
        let snapshot = {
            let cfg = self.state.config.read().await;
            ShadowConfigSnapshot {
                default_agent: cfg.default_agent.clone(),
                workspace_dir: cfg.workspace_dir.clone(),
                workspace_only: cfg.workspace_only,
                model: cfg.model.clone(),
            }
        };

        // 4. Resolve a provider. No provider → no harness evidence (not an
        //    error): the run degrades gracefully.
        let Ok(provider) = self
            .state
            .infra
            .model_router
            .create_default_provider()
            .await
        else {
            debug!(
                "real-turn verifier: no default provider available; no evidence for {}",
                subject.path
            );
            return Ok(None);
        };

        // 5. Build the baseline agent from the current config and a candidate
        //    agent with the proposed value applied to the matching field.
        let mut candidate_cfg = snapshot.default_agent.clone();
        if !apply_scalar_to_config(&mut candidate_cfg, &subject.path, &subject.proposed) {
            debug!("real-turn verifier: unknown scalar path '{}'; no evidence", subject.path);
            return Ok(None);
        }
        let baseline_agent =
            match self.build_agent(provider.clone(), &snapshot, snapshot.default_agent.clone()) {
                Ok(a) => a,
                Err(e) => {
                    warn!(
                        "real-turn verifier: failed to build baseline agent for {}: {}",
                        subject.path, e
                    );
                    return Ok(None);
                }
            };
        let candidate_agent = match self.build_agent(provider, &snapshot, candidate_cfg) {
            Ok(a) => a,
            Err(e) => {
                warn!(
                    "real-turn verifier: failed to build candidate agent for {}: {}",
                    subject.path, e
                );
                return Ok(None);
            }
        };

        // 6. Replay every sampled turn through both harnesses, recording
        //    per-turn pass/fail (pass_rate > 0.0) for each version.
        let baseline_harness = EvalHarness::new(baseline_agent, None);
        let candidate_harness = EvalHarness::new(candidate_agent, None);
        let mut baseline_passes = Vec::with_capacity(turns.len());
        let mut candidate_passes = Vec::with_capacity(turns.len());
        for turn in &turns {
            let task = crate::eval::EvalTask {
                id: format!("real_shadow_{}", turn.turn_id),
                input: turn.input.clone(),
                ..Default::default()
            };
            match baseline_harness.run(task.clone(), self.trials).await {
                Ok(summary) => baseline_passes.push(summary.pass_rate > 0.0),
                Err(e) => {
                    warn!(
                        "real-turn verifier: baseline run failed for turn '{}': {}",
                        turn.turn_id, e
                    );
                    return Ok(None);
                }
            }
            match candidate_harness.run(task, self.trials).await {
                Ok(summary) => candidate_passes.push(summary.pass_rate > 0.0),
                Err(e) => {
                    warn!(
                        "real-turn verifier: candidate run failed for turn '{}': {}",
                        turn.turn_id, e
                    );
                    return Ok(None);
                }
            }
        }

        // 7. Bootstrap the comparison and return the verdict. A candidate
        //    worse than the baseline surfaces as `ComparisonVerdict::Regressed`
        //    inside the returned [`CandidateVerdict`].
        let comparison =
            compare_versions(&baseline_passes, &candidate_passes, self.iterations, self.confidence);
        Ok(Some(CandidateVerdict {
            comparison,
            baseline_trials: baseline_passes.len(),
            candidate_trials: candidate_passes.len(),
            suite_id: ONLINE_SHADOW_SUITE_ID.to_string(),
        }))
    }
}

/// Snapshot of the config fields the verifier needs to build agents. Cloned
/// under the config lock and dropped before any `.await` (mirrors
/// `crate::eval::verdict::HarnessCandidateVerifier`).
struct ShadowConfigSnapshot {
    default_agent: AgentConfig,
    workspace_dir: Option<std::path::PathBuf>,
    workspace_only: bool,
    model: String,
}

/// Apply a proposed scalar value to the matching [`AgentConfig`] field.
/// Returns `false` for paths the scalar verifier does not know (structural
/// targets such as tool descriptions / system prompts have no scalar field and
/// therefore produce no harness evidence). Mirrors
/// `crate::eval::verdict::apply_scalar_to_config`.
fn apply_scalar_to_config(cfg: &mut AgentConfig, path: &str, proposed: &serde_json::Value) -> bool {
    let Some(num) = proposed.as_f64() else {
        return false;
    };
    match path {
        "default_agent.temperature" => {
            cfg.temperature = num as f32;
            true
        }
        "default_agent.max_tokens" => {
            cfg.max_tokens = num as u32;
            true
        }
        "default_agent.max_context_tokens" => {
            cfg.max_context_tokens = num as usize;
            true
        }
        "default_agent.max_concurrent_tools" => {
            cfg.max_concurrent_tools = num as usize;
            true
        }
        _ => false,
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

    fn subject() -> VerdictSubject {
        VerdictSubject {
            path: "default_agent.temperature".to_string(),
            current: serde_json::json!(0.7),
            proposed: serde_json::json!(0.8),
        }
    }

    #[tokio::test]
    async fn real_turn_verifier_without_store_reports_no_evidence() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        let verifier = RealTurnCandidateVerifier::new(state, None, 10, 1, 1000, 0.95);
        // `Ok(None)` = "no evidence" (the gate treats it as a conservative
        // reject rather than a pass).
        assert!(verifier.verify(&subject()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn real_turn_verifier_with_empty_store_reports_no_evidence() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state_with_store(
                crate::gateway::GatewayConfig::default(),
            )
            .await,
        );
        let store = state.infra.sample_store.clone();
        assert!(store.is_some(), "test state should wire an in-memory sample store");
        let verifier = RealTurnCandidateVerifier::new(state, store, 10, 1, 1000, 0.95);
        // Empty pool → no replay turns → no evidence.
        assert!(verifier.verify(&subject()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn real_turn_verifier_constructor_sanitizes_counts() {
        let state = Arc::new(
            crate::gateway::state_tests::make_test_state(crate::gateway::GatewayConfig::default())
                .await,
        );
        let verifier = RealTurnCandidateVerifier::new(state, None, 10, 0, 0, 0.95);
        assert_eq!(verifier.trials, 1);
        assert_eq!(verifier.iterations, 1);
    }
}
