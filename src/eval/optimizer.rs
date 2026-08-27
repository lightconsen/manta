//! Scalar optimizer (§十二 可调参).
//!
//! A background loop (or manual `eval.optimizer.run`) that probes the global
//! default-agent scalar parameters — temperature, token budget, context
//! budget, concurrent-tool cap — and hot-updates them with the same optimistic
//! concurrency machinery as the WS `config.set` handler. Every apply/reject is
//! recorded in `decision_traces` so the loop is auditable and replayable.
//!
//! Phase 4 layers the guardrails (shadow eval, auto-rollback, cost cap,
//! circuit breaker) between candidate generation and application; today a
//! candidate that passes the search-space fence is applied directly.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::error::SyscityError;
use crate::eval::apply_patch::{
    applied_evidence, apply_optimizer_patch, conflict_evidence, OptimizerPatch, PatchOutcome,
};
use crate::eval::decision_trace::{RecordTraceParams, TraceKind, TraceStatus};
use crate::eval::guardrail::{CircuitBreaker, OnlineSignalShadowEvaluator, ShadowEvaluator};
use crate::gateway::apply_config::read_config_scalar;
use crate::gateway::config::ScalarOptimizerConfig;
use crate::gateway::{config_revision, GatewayConfig, GatewayState};

/// A proposed scalar change for the default agent.
#[derive(Debug, Clone)]
pub struct ScalarCandidate {
    pub path: &'static str,
    pub current: f64,
    pub proposed: f64,
    pub reason: String,
}

/// A patch that was successfully applied.
#[derive(Debug, Clone, Serialize)]
pub struct AppliedPatch {
    pub path: String,
    pub from: f64,
    pub to: f64,
    pub new_revision: String,
}

/// A patch that was rejected (conflict / unknown path).
#[derive(Debug, Clone, Serialize)]
pub struct RejectedPatch {
    pub path: String,
    pub reason: String,
}

/// Result of a guardrail-triggered rollback (Phase 4 自动回滚).
#[derive(Debug, Clone, Serialize)]
pub struct RollbackReport {
    pub subject: String,
    pub from: f64,
    pub to: f64,
    pub new_revision: String,
    pub reason: String,
}

/// Result of one optimizer run.
#[derive(Debug, Clone, Serialize)]
pub struct OptimizerRunReport {
    pub run_id: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub candidates_generated: usize,
    pub applied: Vec<AppliedPatch>,
    pub rejected: Vec<RejectedPatch>,
    /// `completed` | `disabled` | `revision_conflict` | `no_viable_candidate`
    /// | `partial`.
    pub reason: String,
}

/// Per-run parameters (overridable from the WS surface).
#[derive(Clone, Default)]
pub struct OptimizerRunParams {
    /// Override `ScalarOptimizerConfig.max_steps`. `None` = use config.
    pub max_steps: Option<u32>,
    /// Bypass the `enabled` switch (used by `eval.optimizer.resume`).
    pub force: bool,
    /// Shadow-evaluator override. `None` = derive the default from the
    /// guardrail config (online-signal evaluator when guardrails are enabled).
    pub shadow: Option<Arc<dyn ShadowEvaluator>>,
}

/// Live run status surfaced by `eval.optimizer.status`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct OptimizerRunStatus {
    pub running: bool,
    pub last_run_at: Option<i64>,
    pub last_report: Option<OptimizerRunReport>,
    pub last_error: Option<String>,
}

/// Shared runtime state for the optimizer (one per gateway).
#[derive(Debug, Default)]
pub struct OptimizerRuntime {
    pub status: RwLock<OptimizerRunStatus>,
    /// Circuit-breaker escape hatch: when set, the scheduled loop skips runs
    /// until `eval.optimizer.resume` clears it (Phase 4 guardrail hook).
    pub paused: AtomicBool,
    /// Trip-counter for consecutive guardrail failures / rollbacks (Phase 4).
    pub breaker: CircuitBreaker,
}

/// How a scalar path is perturbed when generating probe candidates.
#[derive(Debug, Clone, Copy)]
enum Perturb {
    /// Add/subtract a fixed delta.
    Delta(f64),
    /// Multiply/divide by a factor.
    Factor(f64),
    /// Add/subtract an integer step.
    Step(f64),
}

/// Search-space definition for an optimizable scalar path.
struct PathSpec {
    path: &'static str,
    bounds: [f64; 2],
    perturb: Perturb,
}

/// The search space the optimizer is allowed to move (安全区域锁定).
fn path_specs(oc: &ScalarOptimizerConfig) -> Vec<PathSpec> {
    vec![
        PathSpec {
            path: "default_agent.temperature",
            bounds: oc.temperature_bounds,
            perturb: Perturb::Delta(oc.delta),
        },
        PathSpec {
            path: "default_agent.max_tokens",
            bounds: [256.0, 16384.0],
            perturb: Perturb::Factor(1.5),
        },
        PathSpec {
            path: "default_agent.max_context_tokens",
            bounds: [8192.0, 65536.0],
            perturb: Perturb::Factor(1.5),
        },
        PathSpec {
            path: "default_agent.max_concurrent_tools",
            bounds: [1.0, 16.0],
            perturb: Perturb::Step(1.0),
        },
    ]
}

/// Generate probe candidates from the current config. Values that are already
/// at the fence are skipped, and a candidate identical to the current value is
/// never produced (so a run cannot apply a no-op).
pub fn generate_candidates(
    cfg: &GatewayConfig,
    oc: &ScalarOptimizerConfig,
) -> Vec<ScalarCandidate> {
    let mut out = Vec::new();
    for spec in path_specs(oc) {
        let Some(current) = read_config_scalar(cfg, spec.path) else {
            continue;
        };
        let (up, down) = match spec.perturb {
            Perturb::Delta(d) => (current + d, current - d),
            Perturb::Factor(f) => (current * f, current / f),
            Perturb::Step(s) => (current + s, current - s),
        };
        for proposed in [up, down] {
            let clamped = proposed.clamp(spec.bounds[0], spec.bounds[1]);
            // Round so f32-stored scalars produce stable, readable values
            // (0.7f32 as f64 + 0.1 ≈ 0.79999… would otherwise drift).
            let rounded = (clamped * 10_000.0).round() / 10_000.0;
            if (rounded - current).abs() > 1e-6 {
                out.push(ScalarCandidate {
                    path: spec.path,
                    current,
                    proposed: rounded,
                    reason: format!(
                        "{} {:.2} → {:.2} (within [{:.2}, {:.2}])",
                        spec.path, current, rounded, spec.bounds[0], spec.bounds[1]
                    ),
                });
            }
        }
    }
    out
}

/// Parse a cadence string ("30m", "1h", "manual") into an interval. Returns
/// `None` for "manual"/"off"/"never" (no scheduled loop) or an unparseable
/// string.
pub fn parse_cadence(s: &str) -> Option<Duration> {
    let s = s.trim().to_lowercase();
    if s.is_empty() || matches!(s.as_str(), "manual" | "off" | "never") {
        return None;
    }
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let num: u64 = s[..split].parse().ok()?;
    let unit = s[split..].trim();
    let secs = match unit {
        "" | "s" | "sec" | "secs" => num,
        "m" | "min" | "mins" => num * 60,
        "h" | "hr" | "hrs" => num * 3600,
        "d" | "day" | "days" => num * 86_400,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

/// The scalar optimizer. Stateless apart from the shared [`OptimizerRuntime`].
pub struct ScalarOptimizer {
    runtime: Arc<OptimizerRuntime>,
}

impl ScalarOptimizer {
    pub fn new(runtime: Arc<OptimizerRuntime>) -> Self {
        Self { runtime }
    }

    pub fn runtime(&self) -> &Arc<OptimizerRuntime> {
        &self.runtime
    }

    /// Run one optimization pass. Applies at most `max_steps` candidates via
    /// CAS; every apply/reject is recorded in `decision_traces`. Safe to call
    /// concurrently — the CAS fast-fail means a losing run simply records a
    /// reject and stops.
    pub async fn run(
        &self,
        state: Arc<GatewayState>,
        params: OptimizerRunParams,
    ) -> OptimizerRunReport {
        let started_at = now_ms();
        let run_id = uuid::Uuid::new_v4().to_string();
        self.runtime.status.write().await.running = true;

        // Snapshot guardrail-relevant config so the config lock is never held
        // across a CAS apply.
        let (enabled, max_steps, guard_enabled, cost_guard_enabled, cooldown, threshold, evaluator) = {
            let cfg = state.config.read().await;
            let oc = &cfg.eval.optimizer;
            let guard = &oc.guardrails;
            let evaluator: Option<Arc<dyn ShadowEvaluator>> = match &params.shadow {
                Some(e) => Some(e.clone()),
                None if guard.enabled => Some(Arc::new(OnlineSignalShadowEvaluator::new(
                    state.infra.feedback_store.clone(),
                    state.infra.pending_badcase_store.clone(),
                    guard.max_down_ratio,
                    guard.min_votes,
                    guard.max_online_risks,
                    (guard.window_hours as i64) * 3600 * 1000,
                ))),
                None => None,
            };
            (
                oc.enabled,
                params.max_steps.unwrap_or(oc.max_steps).max(1) as usize,
                guard.enabled,
                guard.cost_guard_enabled,
                Duration::from_secs(guard.cooldown_secs),
                guard.max_consecutive_failures.max(1),
                evaluator,
            )
        };

        if !enabled && !params.force {
            debug!("Scalar optimizer run skipped: disabled");
            let report = OptimizerRunReport {
                run_id,
                started_at,
                finished_at: now_ms(),
                candidates_generated: 0,
                applied: Vec::new(),
                rejected: Vec::new(),
                reason: "disabled".to_string(),
            };
            self.finish(&report, None).await;
            return report;
        }

        // Phase 4 guardrail: an open circuit breaker skips the run entirely.
        if guard_enabled && self.runtime.breaker.is_open(cooldown).await {
            debug!("Scalar optimizer run skipped: circuit breaker open");
            let report = OptimizerRunReport {
                run_id,
                started_at,
                finished_at: now_ms(),
                candidates_generated: 0,
                applied: Vec::new(),
                rejected: Vec::new(),
                reason: "circuit_open".to_string(),
            };
            self.finish(&report, None).await;
            return report;
        }

        let candidates = {
            let cfg = state.config.read().await;
            let oc = &cfg.eval.optimizer;
            generate_candidates(&cfg, oc)
        };
        let generated = candidates.len();
        let mut base_revision = {
            let cfg = state.config.read().await;
            config_revision(&cfg)
        };
        let mut applied: Vec<AppliedPatch> = Vec::new();
        let mut rejected: Vec<RejectedPatch> = Vec::new();
        let mut reason = "completed".to_string();

        for cand in candidates.into_iter().take(max_steps) {
            // Gate 1 — cost cap (§十二 护栏 · 成本封顶).
            if guard_enabled && cost_guard_enabled && state.agents.cost_guard.is_exceeded() {
                self.record_trace(
                    &state,
                    TraceKind::GateFail,
                    cand.path.to_string(),
                    json!({ "path": cand.path, "from": cand.current, "to": cand.proposed }),
                    json!({ "run_id": run_id, "gate": "cost_guard", "verdict": "fail", "reason": "daily budget exceeded" }),
                    TraceStatus::Rejected,
                )
                .await;
                rejected.push(RejectedPatch {
                    path: cand.path.to_string(),
                    reason: "cost_guard_exceeded".to_string(),
                });
                reason = "cost_guard_exceeded".to_string();
                self.trip_breaker(&state, threshold).await;
                break;
            }

            // Gate 2 — shadow eval (§十二 护栏 · shadow eval).
            if guard_enabled {
                if let Some(ev) = &evaluator {
                    let pass = match ev.evaluate(&cand).await {
                        Ok(true) => true,
                        Ok(false) => false,
                        Err(e) => {
                            warn!("Shadow evaluator failed for {}: {}", cand.path, e);
                            false
                        }
                    };
                    if !pass {
                        self.record_trace(
                            &state,
                            TraceKind::GateFail,
                            cand.path.to_string(),
                            json!({ "path": cand.path, "from": cand.current, "to": cand.proposed }),
                            json!({ "run_id": run_id, "gate": "shadow_eval", "verdict": "fail", "reason": "candidate degraded vs baseline" }),
                            TraceStatus::Rejected,
                        )
                        .await;
                        rejected.push(RejectedPatch {
                            path: cand.path.to_string(),
                            reason: "shadow_fail".to_string(),
                        });
                        reason = "shadow_fail".to_string();
                        self.trip_breaker(&state, threshold).await;
                        break;
                    }
                }
            }

            let patch = OptimizerPatch {
                path: cand.path.to_string(),
                value: json!(cand.proposed),
            };
            match apply_optimizer_patch(&state, &patch, &base_revision).await {
                PatchOutcome::Applied { new_revision } => {
                    self.record_trace(
                        &state,
                        TraceKind::OptimizerApply,
                        cand.path.to_string(),
                        json!({ "path": cand.path, "from": cand.current, "to": cand.proposed }),
                        applied_evidence(
                            &run_id,
                            cand.current,
                            cand.proposed,
                            &base_revision,
                            &new_revision,
                        ),
                        TraceStatus::Applied,
                    )
                    .await;
                    applied.push(AppliedPatch {
                        path: cand.path.to_string(),
                        from: cand.current,
                        to: cand.proposed,
                        new_revision: new_revision.clone(),
                    });
                    base_revision = new_revision;

                    // Gate 3 — post-apply online-signal anomaly → auto-rollback
                    // (§十二 护栏 · 自动回滚). The pre-apply gate (Gate 2) only
                    // rejects candidates against the *current* signal window;
                    // this re-checks signals after the change is live and
                    // reverts it if they degraded. `rollback` records the
                    // `rollback` trace and counts the breaker failure. A clean
                    // apply still closes the breaker.
                    if guard_enabled && self.post_apply_anomaly(&evaluator, &cand).await {
                        match self
                            .rollback(state.clone(), cand.path, "online_anomaly_rollback")
                            .await
                        {
                            Ok(rb) => {
                                warn!(
                                    "Auto-rolled back {} after online-signal anomaly ({}): {} → {}",
                                    cand.path, rb.reason, rb.from, rb.to
                                );
                                applied.pop();
                            }
                            Err(e) => {
                                warn!("Auto-rollback failed for {}: {}", cand.path, e);
                                // The signal is still anomalous: count the
                                // failure and stop so no further candidates are
                                // applied under a degraded signal window.
                                self.trip_breaker(&state, threshold).await;
                            }
                        }
                        rejected.push(RejectedPatch {
                            path: cand.path.to_string(),
                            reason: "online_anomaly_rollback".to_string(),
                        });
                        reason = "online_anomaly_rollback".to_string();
                        break;
                    }
                    self.runtime.breaker.record_success().await;
                }
                PatchOutcome::Conflict { current } => {
                    self.record_trace(
                        &state,
                        TraceKind::OptimizerReject,
                        cand.path.to_string(),
                        json!({ "path": cand.path, "from": cand.current, "to": cand.proposed }),
                        conflict_evidence(&run_id, &current),
                        TraceStatus::Rejected,
                    )
                    .await;
                    rejected.push(RejectedPatch {
                        path: cand.path.to_string(),
                        reason: "revision_conflict".to_string(),
                    });
                    reason = "revision_conflict".to_string();
                    break;
                }
                PatchOutcome::UnknownPath => {
                    rejected.push(RejectedPatch {
                        path: cand.path.to_string(),
                        reason: "unknown_path".to_string(),
                    });
                    reason = "partial".to_string();
                }
            }
        }

        if applied.is_empty() && rejected.is_empty() {
            reason = "no_viable_candidate".to_string();
        }

        let report = OptimizerRunReport {
            run_id,
            started_at,
            finished_at: now_ms(),
            candidates_generated: generated,
            applied,
            rejected,
            reason,
        };
        self.finish(&report, None).await;
        report
    }

    /// Roll back a previously applied optimizer change for `subject`, writing
    /// the pre-apply baseline back through the same CAS machinery (§十二 护栏 ·
    /// 自动回滚). The baseline is read from the most recent `optimizer_apply`
    /// decision trace, so a rollback is always attributable. A rollback counts
    /// as a failure for the circuit breaker.
    pub async fn rollback(
        &self,
        state: Arc<GatewayState>,
        subject: &str,
        reason: &str,
    ) -> Result<RollbackReport, SyscityError> {
        let Some(store) = state.infra.decision_trace_store.as_ref() else {
            return Err(SyscityError::Internal(
                "decision trace store not initialized; cannot roll back".to_string(),
            ));
        };
        let traces = store
            .list(Some(TraceKind::OptimizerApply), 100)
            .await
            .map_err(|e| SyscityError::Storage {
                context: "failed to read apply traces for rollback".to_string(),
                details: e.to_string(),
            })?;
        let apply = traces
            .iter()
            .find(|t| t.subject == subject)
            .ok_or_else(|| SyscityError::NotFound {
                resource: format!("prior optimizer apply for {subject}"),
            })?;
        let baseline = apply.payload["from"].as_f64().ok_or_else(|| {
            SyscityError::Internal(format!(
                "apply trace for {subject} has no numeric 'from' baseline"
            ))
        })?;

        let current = {
            let cfg = state.config.read().await;
            read_config_scalar(&cfg, subject)
                .ok_or_else(|| SyscityError::Internal(format!("unknown scalar path {subject}")))?
        };
        let base_revision = {
            let cfg = state.config.read().await;
            config_revision(&cfg)
        };

        let patch = OptimizerPatch {
            path: subject.to_string(),
            value: json!(baseline),
        };
        match apply_optimizer_patch(&state, &patch, &base_revision).await {
            PatchOutcome::Applied { new_revision } => {
                self.record_trace(
                    &state,
                    TraceKind::Rollback,
                    subject.to_string(),
                    json!({ "path": subject, "from": current, "to": baseline }),
                    json!({ "reason": reason, "base_revision": base_revision, "new_revision": new_revision }),
                    TraceStatus::Applied,
                )
                .await;
                self.trip_breaker(&state, self.failure_threshold(&state).await)
                    .await;
                Ok(RollbackReport {
                    subject: subject.to_string(),
                    from: current,
                    to: baseline,
                    new_revision,
                    reason: reason.to_string(),
                })
            }
            PatchOutcome::Conflict { current } => {
                self.record_trace(
                    &state,
                    TraceKind::OptimizerReject,
                    subject.to_string(),
                    json!({ "path": subject, "from": current, "to": baseline }),
                    conflict_evidence("rollback", &current),
                    TraceStatus::Rejected,
                )
                .await;
                Err(SyscityError::Internal(format!(
                    "rollback conflicted; current revision {current}"
                )))
            }
            PatchOutcome::UnknownPath => {
                Err(SyscityError::Internal(format!("unknown scalar path {subject}")))
            }
        }
    }

    /// Current circuit-breaker failure threshold from config.
    async fn failure_threshold(&self, state: &Arc<GatewayState>) -> u32 {
        let cfg = state.config.read().await;
        cfg.eval
            .optimizer
            .guardrails
            .max_consecutive_failures
            .max(1)
    }

    /// Count a guardrail failure; when the threshold is reached, trip the
    /// breaker and pause the scheduler. Only armed when guardrails are enabled.
    async fn trip_breaker(&self, state: &Arc<GatewayState>, threshold: u32) {
        let guard_enabled = {
            let cfg = state.config.read().await;
            cfg.eval.optimizer.guardrails.enabled
        };
        if !guard_enabled {
            return;
        }
        if self.runtime.breaker.record_failure(threshold).await {
            state.infra.optimizer.paused.store(true, Ordering::SeqCst);
            warn!(
                "Optimizer circuit breaker tripped: pausing auto-apply (resume via eval.optimizer.resume)"
            );
        }
    }

    /// Post-apply online-signal anomaly check (§十二 护栏 · 自动回滚). Returns
    /// `true` when the shadow evaluator reports an anomalous online signal for
    /// the just-applied candidate. An `is_anomalous` error is `warn!`ed and
    /// treated as not-anomalous — a signal-store failure must not veto an
    /// otherwise clean apply.
    async fn post_apply_anomaly(
        &self,
        evaluator: &Option<Arc<dyn ShadowEvaluator>>,
        cand: &ScalarCandidate,
    ) -> bool {
        let Some(ev) = evaluator else {
            return false;
        };
        match ev.is_anomalous().await {
            Ok(true) => true,
            Ok(false) => false,
            Err(e) => {
                warn!("Post-apply anomaly check failed for {}: {}", cand.path, e);
                false
            }
        }
    }

    /// Persist run status. `err` records a failure on the status surface.
    async fn finish(&self, report: &OptimizerRunReport, err: Option<String>) {
        let mut st = self.runtime.status.write().await;
        st.running = false;
        st.last_run_at = Some(report.finished_at);
        st.last_report = Some(report.clone());
        st.last_error = err;
    }

    /// Record a decision trace. No-op when the decision-trace store is not
    /// wired (e.g. tests without SQLite storage).
    async fn record_trace(
        &self,
        state: &Arc<GatewayState>,
        kind: TraceKind,
        subject: String,
        payload: serde_json::Value,
        evidence: serde_json::Value,
        status: TraceStatus,
    ) {
        let Some(store) = state.infra.decision_trace_store.as_ref() else {
            return;
        };
        if let Err(e) = store
            .record(&RecordTraceParams {
                kind,
                subject,
                payload,
                evidence,
                status,
            })
            .await
        {
            warn!("Failed to record decision trace: {}", e);
        }
    }
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state_with_store;
    use crate::gateway::GatewayConfig;
    use async_trait::async_trait;

    fn enabled_config() -> GatewayConfig {
        let mut cfg = GatewayConfig::default();
        cfg.eval.optimizer.enabled = true;
        cfg.eval.optimizer.max_steps = 1;
        cfg.eval.optimizer.delta = 0.1;
        cfg
    }

    #[test]
    fn generates_in_bounds_candidates() {
        let cfg = GatewayConfig::default();
        let oc = &cfg.eval.optimizer;
        let cands = generate_candidates(&cfg, oc);
        assert!(!cands.is_empty());
        for c in &cands {
            assert!((c.proposed - c.current).abs() > 1e-6, "no-op candidate");
            assert!(
                c.proposed >= 0.0 && c.proposed <= 1.5 || c.path != "default_agent.temperature"
            );
        }
        // Temperature default 0.7 ± 0.1 → 0.8 and 0.6.
        let temps: Vec<_> = cands
            .iter()
            .filter(|c| c.path == "default_agent.temperature")
            .map(|c| c.proposed)
            .collect();
        assert!(temps.contains(&0.8));
        assert!(temps.contains(&0.6));
    }

    #[test]
    fn skips_candidates_at_fence() {
        let mut cfg = GatewayConfig::default();
        // Push temperature to the upper fence: no upward candidate remains.
        cfg.default_agent.temperature = 1.5;
        let cands = generate_candidates(&cfg, &cfg.eval.optimizer);
        let temps: Vec<_> = cands
            .iter()
            .filter(|c| c.path == "default_agent.temperature")
            .map(|c| c.proposed)
            .collect();
        assert!(!temps.contains(&1.5), "fence value must not be re-proposed");
        assert!(temps.contains(&1.4), "downward probe still viable");
    }

    #[test]
    fn parses_cadence_strings() {
        assert_eq!(parse_cadence("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(parse_cadence("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_cadence("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_cadence("manual"), None);
        assert_eq!(parse_cadence("off"), None);
        assert_eq!(parse_cadence("garbage"), None);
    }

    #[tokio::test]
    async fn run_applies_cas_and_records_trace() {
        let state = Arc::new(make_test_state_with_store(enabled_config()).await);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));

        let before = {
            let cfg = state.config.read().await;
            (cfg.default_agent.temperature, config_revision(&cfg))
        };
        let report = optimizer
            .run(state.clone(), OptimizerRunParams::default())
            .await;

        assert_eq!(report.reason, "completed");
        assert_eq!(report.applied.len(), 1, "one candidate applied per step");
        let cfg = state.config.read().await;
        assert_ne!(cfg.default_agent.temperature, before.0);
        assert_ne!(config_revision(&cfg), before.1, "revision must change");

        // Decision trace recorded with applied status.
        let traces = state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].kind, TraceKind::OptimizerApply);
        assert_eq!(traces[0].status, TraceStatus::Applied);
        assert!(traces[0].evidence["new_revision"].is_string());
    }

    #[tokio::test]
    async fn run_when_disabled_is_noop() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));
        let report = optimizer
            .run(state.clone(), OptimizerRunParams::default())
            .await;
        assert_eq!(report.reason, "disabled");
        assert!(report.applied.is_empty());
        assert!(state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn force_bypasses_disabled_switch() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));
        let report = optimizer
            .run(
                state.clone(),
                OptimizerRunParams {
                    force: true,
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(report.reason, "completed");
        assert_eq!(report.applied.len(), 1);
        let traces = state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap();
        assert_eq!(traces.len(), 1, "force run must record its apply");
    }

    // ── Phase 4 guardrails ───────────────────────────────────────────────

    fn guarded_config() -> GatewayConfig {
        let mut cfg = enabled_config();
        cfg.eval.optimizer.guardrails.enabled = true;
        cfg.eval.optimizer.guardrails.max_consecutive_failures = 2;
        cfg.eval.optimizer.guardrails.cooldown_secs = 300;
        cfg
    }

    struct RejectEvaluator;
    #[async_trait]
    impl ShadowEvaluator for RejectEvaluator {
        async fn evaluate(&self, _c: &ScalarCandidate) -> Result<bool, String> {
            Ok(false)
        }
    }

    struct PassEvaluator;
    #[async_trait]
    impl ShadowEvaluator for PassEvaluator {
        async fn evaluate(&self, _c: &ScalarCandidate) -> Result<bool, String> {
            Ok(true)
        }
    }

    /// Test evaluator: passes the pre-apply shadow gate, then reports an
    /// online-signal anomaly once a candidate has been evaluated — simulating a
    /// regression triggered by the just-applied change.
    #[derive(Default)]
    struct AnomalyAfterApplyEvaluator {
        evaluated: AtomicBool,
    }
    #[async_trait]
    impl ShadowEvaluator for AnomalyAfterApplyEvaluator {
        async fn evaluate(&self, _c: &ScalarCandidate) -> Result<bool, String> {
            self.evaluated.store(true, Ordering::SeqCst);
            Ok(true)
        }
        async fn is_anomalous(&self) -> Result<bool, String> {
            Ok(self.evaluated.load(Ordering::SeqCst))
        }
    }

    #[tokio::test]
    async fn rejecting_shadow_evaluator_blocks_candidate() {
        let state = Arc::new(make_test_state_with_store(guarded_config()).await);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));

        let report = optimizer
            .run(
                state.clone(),
                OptimizerRunParams {
                    shadow: Some(Arc::new(RejectEvaluator)),
                    ..Default::default()
                },
            )
            .await;

        assert_eq!(report.reason, "shadow_fail");
        assert!(report.applied.is_empty(), "degraded candidate must not apply");
        assert_eq!(report.rejected[0].reason, "shadow_fail");

        // A `gate_fail` trace was recorded; the apply never happened.
        let traces = state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].kind, TraceKind::GateFail);
        assert_eq!(traces[0].status, TraceStatus::Rejected);
        assert_eq!(traces[0].evidence["gate"], "shadow_eval");

        // One failure is below the threshold of two — not tripped yet.
        let snap = state.infra.optimizer.breaker.snapshot().await;
        assert_eq!(snap.failures, 1);
        assert!(!snap.tripped);
    }

    #[tokio::test]
    async fn cost_guard_rejects_when_over_budget() {
        let state = Arc::new(make_test_state_with_store(guarded_config()).await);
        // Simulate an exceeded budget.
        state
            .agents
            .cost_guard
            .budget_exceeded
            .store(true, Ordering::Release);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));

        let report = optimizer
            .run(state.clone(), OptimizerRunParams::default())
            .await;

        assert_eq!(report.reason, "cost_guard_exceeded");
        assert!(report.applied.is_empty());
        let traces = state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].kind, TraceKind::GateFail);
        assert_eq!(traces[0].evidence["gate"], "cost_guard");
    }

    #[tokio::test]
    async fn consecutive_failures_trip_breaker_and_pause() {
        let state = Arc::new(make_test_state_with_store(guarded_config()).await);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));
        let params = OptimizerRunParams {
            shadow: Some(Arc::new(RejectEvaluator)),
            ..Default::default()
        };

        let r1 = optimizer.run(state.clone(), params.clone()).await;
        assert_eq!(r1.reason, "shadow_fail");
        assert!(!state.infra.optimizer.paused.load(Ordering::SeqCst));

        let r2 = optimizer.run(state.clone(), params.clone()).await;
        assert_eq!(r2.reason, "shadow_fail");
        assert!(
            state.infra.optimizer.paused.load(Ordering::SeqCst),
            "second consecutive failure must pause auto-apply"
        );
        assert!(
            state
                .infra
                .optimizer
                .breaker
                .is_open(Duration::from_secs(300))
                .await
        );

        // A third run while the breaker is open is skipped entirely.
        let r3 = optimizer.run(state.clone(), params).await;
        assert_eq!(r3.reason, "circuit_open");
        assert!(r3.applied.is_empty());
        assert!(r3.rejected.is_empty());
    }

    #[tokio::test]
    async fn online_anomaly_after_apply_triggers_auto_rollback() {
        let state = Arc::new(make_test_state_with_store(guarded_config()).await);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));

        let before = state.config.read().await.default_agent.temperature;

        let report = optimizer
            .run(
                state.clone(),
                OptimizerRunParams {
                    shadow: Some(Arc::new(AnomalyAfterApplyEvaluator::default())),
                    ..Default::default()
                },
            )
            .await;

        assert_eq!(report.reason, "online_anomaly_rollback");
        assert!(report.applied.is_empty(), "the rolled-back patch must not remain in `applied`");
        assert_eq!(report.rejected.len(), 1);
        assert_eq!(report.rejected[0].reason, "online_anomaly_rollback");

        // The config value is reverted to baseline.
        assert_eq!(
            state.config.read().await.default_agent.temperature,
            before,
            "value must revert to baseline after anomaly rollback"
        );

        // A `rollback` decision trace exists for the applied path.
        let traces = state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap();
        let rb = traces
            .iter()
            .find(|t| t.kind == TraceKind::Rollback)
            .expect("a rollback trace must be recorded");
        assert_eq!(rb.subject, report.rejected[0].path);
        assert_eq!(rb.evidence["reason"], "online_anomaly_rollback");
        assert_eq!(rb.payload["to"], before as f64);
    }

    #[tokio::test]
    async fn resume_clears_breaker_and_recovers() {
        let state = Arc::new(make_test_state_with_store(guarded_config()).await);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));
        let reject = OptimizerRunParams {
            shadow: Some(Arc::new(RejectEvaluator)),
            ..Default::default()
        };

        optimizer.run(state.clone(), reject.clone()).await;
        optimizer.run(state.clone(), reject.clone()).await;
        assert!(state.infra.optimizer.paused.load(Ordering::SeqCst));

        // `eval.optimizer.resume` clears both pause and breaker.
        state.infra.optimizer.paused.store(false, Ordering::SeqCst);
        state.infra.optimizer.breaker.reset().await;
        assert!(
            !state
                .infra
                .optimizer
                .breaker
                .is_open(Duration::from_secs(300))
                .await
        );

        // A passing run then applies and closes the breaker.
        let pass = OptimizerRunParams {
            shadow: Some(Arc::new(PassEvaluator)),
            ..Default::default()
        };
        let report = optimizer.run(state.clone(), pass).await;
        assert_eq!(report.reason, "completed");
        assert_eq!(report.applied.len(), 1);
        let snap = state.infra.optimizer.breaker.snapshot().await;
        assert_eq!(snap.failures, 0);
        assert!(!snap.tripped);
    }

    #[tokio::test]
    async fn rollback_reverts_applied_change_and_records_trace() {
        // Guardrails off so the candidate is applied, then roll it back.
        let state = Arc::new(make_test_state_with_store(enabled_config()).await);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));

        let before = state.config.read().await.default_agent.temperature;
        let report = optimizer
            .run(state.clone(), OptimizerRunParams::default())
            .await;
        assert_eq!(report.applied.len(), 1);
        let after = state.config.read().await.default_agent.temperature;
        assert_ne!(before, after, "candidate must have been applied");

        let rb = optimizer
            .rollback(state.clone(), &report.applied[0].path, "regression")
            .await
            .expect("rollback succeeds");
        assert_eq!(rb.from, after as f64);
        assert_eq!(rb.to, before as f64);

        let reverted = state.config.read().await.default_agent.temperature;
        assert_eq!(reverted, before, "value must revert to baseline");

        // A `rollback` trace was recorded.
        let traces = state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap();
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0].kind, TraceKind::Rollback);
        assert_eq!(traces[0].evidence["reason"], "regression");
        assert_eq!(traces[0].payload["to"], before as f64);
    }

    #[tokio::test]
    async fn rollback_without_prior_apply_errors() {
        let state = Arc::new(make_test_state_with_store(enabled_config()).await);
        let optimizer = Arc::new(ScalarOptimizer::new(state.infra.optimizer.clone()));
        let err = optimizer
            .rollback(state.clone(), "default_agent.temperature", "manual")
            .await
            .unwrap_err();
        assert!(
            matches!(err, SyscityError::NotFound { .. }),
            "no prior apply → NotFound, got {err:?}"
        );
    }
}
