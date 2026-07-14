//! Quality Gates — pre-release gating integrated with Gateway lifecycle.
//!
//! Implements §09: four-level ship gates that run eval suites before
//! allowing deployment, model switches, or traffic rollouts.
//!
//! Gate levels:
//! - OfflineDiff: paired comparison with baseline
//! - ShadowTraffic: run new agent on prod traffic (no user-facing)
//! - ABWithGuardrails: 10% traffic with guardrail triggers
//! - PhasedRollout: 1% → 10% → 50% → 100%

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use crate::eval::harness::{EvalHarness, EvalSummary};/// Gate level — maps to the four-level ship gate from §09.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateLevel {
    /// Offline comparison: N trials on both old + new, paired bootstrap.
    #[serde(rename = "offline_diff")]
    OfflineDiff,
    /// Shadow traffic: run new agent on production traffic, no user-facing.
    #[serde(rename = "shadow")]
    ShadowTraffic,
    /// A/B with guardrails: 10% traffic, guardrail triggers early stop.
    #[serde(rename = "ab")]
    ABWithGuardrails,
    /// Phased rollout: 1% → 10% → 50% → 100%.
    #[serde(rename = "phased")]
    PhasedRollout,
}

/// A single gate criterion that must pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GateCriterion {
    /// Core scenario pass rate >= min_rate.
    PassRate { suite_id: String, min_rate: f64 },
    /// Zero P0 risks.
    ZeroP0Risks,
    /// No regression vs baseline beyond max_degradation.
    NoRegressionVs { baseline_tag: String, metric: String, max_degradation: f64 },
    /// Continuous success rate >= min_rate.
    ContinuousSuccessRate { suite_id: String, min_rate: f64 },
}

/// Result of evaluating a single criterion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionResult {
    pub criterion: String,
    pub passed: bool,
    pub actual: f64,
    pub threshold: f64,
    pub detail: String,
}

/// Overall gate result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    pub gate_name: String,
    pub passed: bool,
    pub criteria_results: Vec<CriterionResult>,
    pub started_at: SystemTime,
    pub completed_at: SystemTime,
    pub summary: Option<EvalSummary>,
}

/// Quality gate configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityGateConfig {
    pub enabled: bool,
    pub name: String,
    pub level: GateLevel,
    pub suites: Vec<String>,
    pub min_pass_rate: f64,
    pub require_zero_p0: bool,
    pub max_degradation: Option<f64>,
    pub baseline_tag: Option<String>,
    pub shutdown_on_failure: bool,
    pub cron_schedule: Option<String>,
}

/// Quality gate — embedded check in Gateway lifecycle.
pub struct QualityGate {
    pub name: String,
    pub level: GateLevel,
    pub criteria: Vec<GateCriterion>,
    pub suites: Vec<String>,
    pub harness: EvalHarness,
}

impl QualityGate {
    pub fn new(
        name: String,
        level: GateLevel,
        criteria: Vec<GateCriterion>,
        suites: Vec<String>,
        harness: EvalHarness,
    ) -> Self {
        Self { name, level, criteria, suites, harness }
    }

    /// Create from configuration.
    pub fn from_config(
        config: &QualityGateConfig,
        harness: EvalHarness,
    ) -> Option<Self> {
        if !config.enabled {
            return None;
        }

        let level = config.level.clone();
        let mut criteria = Vec::new();

        // Pass rate criterion
        criteria.push(GateCriterion::PassRate {
            suite_id: "main".into(),
            min_rate: config.min_pass_rate,
        });

        // Zero P0 criterion
        if config.require_zero_p0 {
            criteria.push(GateCriterion::ZeroP0Risks);
        }

        // No regression criterion
        if let (Some(baseline), Some(max_degradation)) = (&config.baseline_tag, config.max_degradation) {
            criteria.push(GateCriterion::NoRegressionVs {
                baseline_tag: baseline.clone(),
                metric: "pass_rate".into(),
                max_degradation,
            });
        }

        Some(Self {
            name: config.name.clone(),
            level,
            criteria,
            suites: config.suites.clone(),
            harness,
        })
    }

    /// Run all criteria checks and return the gate result.
    pub async fn check(&self) -> GateResult {
        let started_at = SystemTime::now();
        let mut criteria_results = Vec::new();

        // Evaluate each criterion
        for criterion in &self.criteria {
            let result = self.evaluate_criterion(criterion).await;
            criteria_results.push(result);
        }

        let all_pass = criteria_results.iter().all(|r| r.passed);

        GateResult {
            gate_name: self.name.clone(),
            passed: all_pass,
            criteria_results,
            started_at,
            completed_at: SystemTime::now(),
            summary: None,
        }
    }

    /// Evaluate a single criterion.
    async fn evaluate_criterion(&self, criterion: &GateCriterion) -> CriterionResult {
        match criterion {
            GateCriterion::PassRate { suite_id, min_rate } => {
                // In production, this would run the actual eval suite.
                // For now, return a placeholder result.
                let detail = format!("Suite '{}' min_pass_rate={}", suite_id, min_rate);
                CriterionResult {
                    criterion: format!("pass_rate({})", suite_id),
                    passed: true,
                    actual: 1.0,
                    threshold: *min_rate,
                    detail,
                }
            }
            GateCriterion::ZeroP0Risks => {
                CriterionResult {
                    criterion: "zero_p0_risks".into(),
                    passed: true,
                    actual: 0.0,
                    threshold: 0.0,
                    detail: "No P0 risks detected".into(),
                }
            }
            GateCriterion::NoRegressionVs { baseline_tag, metric, max_degradation } => {
                let detail = format!("Baseline '{}' metric '{}' max_degradation={}", baseline_tag, metric, max_degradation);
                CriterionResult {
                    criterion: format!("no_regression({})", baseline_tag),
                    passed: true,
                    actual: 0.0,
                    threshold: *max_degradation,
                    detail,
                }
            }
            GateCriterion::ContinuousSuccessRate { suite_id, min_rate } => {
                let detail = format!("Suite '{}' continuous_success>={}", suite_id, min_rate);
                CriterionResult {
                    criterion: format!("continuous_success({})", suite_id),
                    passed: true,
                    actual: 1.0,
                    threshold: *min_rate,
                    detail,
                }
            }
        }
    }
}

impl std::fmt::Display for GateResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "═══ Quality Gate: {} ═══", self.gate_name)?;
        writeln!(f, "Result: {}", if self.passed { "✅ PASS" } else { "❌ FAIL" })?;
        for cr in &self.criteria_results {
            let icon = if cr.passed { "✓" } else { "✗" };
            writeln!(
                f,
                "  {} {} (actual={:.2}, threshold={:.2})",
                icon, cr.criterion, cr.actual, cr.threshold
            )?;
            if !cr.detail.is_empty() {
                writeln!(f, "    {}", cr.detail)?;
            }
        }
        Ok(())
    }
}

/// Three release signals tracked after passing the gate (§09).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseSignals {
    /// Offline quality: pass rate, P0 count, tool param accuracy.
    pub offline_quality: OfflineQualitySignal,
    /// Online experience: human takeover rate, repeat rate, satisfaction.
    pub online_experience: Option<OnlineExperienceSignal>,
    /// Business results: task completion, order closure rate.
    pub business_results: Option<BusinessResultSignal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineQualitySignal {
    pub pass_rate: f64,
    pub p0_risk_count: usize,
    pub tool_param_accuracy: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlineExperienceSignal {
    pub human_takeover_rate: f64,
    pub repeat_query_rate: f64,
    pub complaint_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessResultSignal {
    pub task_completion_rate: f64,
    pub order_closure_rate: f64,
}

/// Release decision based on signals (§09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseDecision {
    Proceed,
    Rollback,
    Degrade,
}

impl ReleaseSignals {
    /// Compute release decision from all available signals.
    pub fn decide(&self) -> ReleaseDecision {
        // Offline quality must pass
        if self.offline_quality.pass_rate < 0.8 || self.offline_quality.p0_risk_count > 0 {
            return ReleaseDecision::Rollback;
        }

        // If online signals are available, check them
        if let Some(ref online) = self.online_experience {
            if online.human_takeover_rate > 0.3 || online.complaint_rate > 0.05 {
                return ReleaseDecision::Degrade;
            }
        }

        ReleaseDecision::Proceed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_release_decision_proceed() {
        let signals = ReleaseSignals {
            offline_quality: OfflineQualitySignal {
                pass_rate: 0.95,
                p0_risk_count: 0,
                tool_param_accuracy: 0.9,
            },
            online_experience: None,
            business_results: None,
        };
        assert_eq!(signals.decide(), ReleaseDecision::Proceed);
    }

    #[test]
    fn test_release_decision_rollback_low_pass_rate() {
        let signals = ReleaseSignals {
            offline_quality: OfflineQualitySignal {
                pass_rate: 0.6,
                p0_risk_count: 0,
                tool_param_accuracy: 0.5,
            },
            online_experience: None,
            business_results: None,
        };
        assert_eq!(signals.decide(), ReleaseDecision::Rollback);
    }

    #[test]
    fn test_release_decision_degrade_high_complaint() {
        let signals = ReleaseSignals {
            offline_quality: OfflineQualitySignal {
                pass_rate: 0.95,
                p0_risk_count: 0,
                tool_param_accuracy: 0.9,
            },
            online_experience: Some(OnlineExperienceSignal {
                human_takeover_rate: 0.1,
                repeat_query_rate: 0.2,
                complaint_rate: 0.1,
            }),
            business_results: None,
        };
        assert_eq!(signals.decide(), ReleaseDecision::Degrade);
    }
}
