//! Layered scoring engine — Coarse → Fine → Human Review.
//!
//! Implements the three-tier screening architecture from §06-9:
//! 1. Coarse (Code Scorer) — fast deterministic checks via GoalCondition
//! 2. Fine (LLM Judge) — semantic evaluation via Critic
//! 3. Human Review — routing for low-confidence / conflict cases

use serde::{Deserialize, Serialize};

use crate::agent::reflection::critic::Critic;
use crate::agent::reflection::types::{Critique, QualityCriteria};
use crate::eval::rca::ProblemPhenomenon;
use crate::goal::condition::{CheckResult, GoalCondition};

/// Which screening layer produced the verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreeningLayer {
    /// Passed or failed by rule Scorer (cheap, deterministic).
    Coarse,
    /// Passed or failed by LLM Judge (semantic).
    Fine,
    /// Needs human review (conflict / low confidence).
    HumanReview,
}

/// Final verdict from the scoring pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Pass,
    Fail,
    /// Not enough information to decide — route to human.
    InsufficientInfo,
}

/// Output of the layered scoring pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringOutput {
    pub verdict: Verdict,
    pub score: f64,
    /// Phenomenon classification (for downstream RCA).
    pub problem_category: Option<ProblemPhenomenon>,
    /// Confidence in the verdict (0.0–1.0).
    pub confidence: f64,
    /// Human-readable judgment basis.
    pub judgment_basis: String,
    /// Which layer produced this output.
    pub screening_layer: ScreeningLayer,
}

/// Configuration for the layered scorer.
#[derive(Debug, Clone)]
pub struct ScorerConfig {
    /// Confidence threshold below which fine scoring is skipped (early pass).
    pub coarse_pass_threshold: f64,
    /// Confidence threshold below which fine scoring is skipped (early fail).
    pub coarse_fail_threshold: f64,
    /// Fine scorer confidence below which human review is triggered.
    pub fine_min_confidence: f64,
}

impl Default for ScorerConfig {
    fn default() -> Self {
        Self {
            coarse_pass_threshold: 0.9,
            coarse_fail_threshold: 0.3,
            fine_min_confidence: 0.6,
        }
    }
}

/// Layered scoring engine.
///
/// Implements the three-tier screening from §06-9:
/// ```
/// Coarse (GoalCondition) → Fine (Critic) → HumanReview (routing)
/// ```
pub struct LayeredScorer {
    critic: Option<Critic>,
    config: ScorerConfig,
}

impl LayeredScorer {
    pub fn new(critic: Option<Critic>, config: ScorerConfig) -> Self {
        Self { critic, config }
    }

    /// Run the full layered scoring pipeline.
    pub async fn score(
        &self,
        conditions: &[GoalCondition],
        criteria: Option<&QualityCriteria>,
        _response: &str,
        trajectory: &str,
    ) -> ScoringOutput {
        // ── Coarse layer: GoalCondition checks ─────────────────────
        let mut condition_results = Vec::new();
        for cond in conditions {
            let result = cond.check().await;
            condition_results.push(result);
        }

        let all_passed = condition_results.iter().all(|r| r.passed);
        let all_failed = condition_results.iter().all(|r| !r.passed);
        let pass_ratio = if condition_results.is_empty() {
            1.0
        } else {
            condition_results.iter().filter(|r| r.passed).count() as f64
                / condition_results.len() as f64
        };

        let detail = condition_results
            .iter()
            .map(|r| format!("{}: {}", r.condition, if r.passed { "PASS" } else { "FAIL" }))
            .collect::<Vec<_>>()
            .join("; ");

        // If all conditions pass with high confidence, early pass
        if all_passed && pass_ratio >= self.config.coarse_pass_threshold {
            return ScoringOutput {
                verdict: Verdict::Pass,
                score: pass_ratio,
                problem_category: None,
                confidence: pass_ratio,
                judgment_basis: format!("Coarse scorer: all conditions pass. {}", detail),
                screening_layer: ScreeningLayer::Coarse,
            };
        }

        // If all conditions fail with high confidence, early fail
        if all_failed && pass_ratio <= self.config.coarse_fail_threshold {
            return ScoringOutput {
                verdict: Verdict::Fail,
                score: pass_ratio,
                problem_category: detect_category_from_conditions(&condition_results),
                confidence: 1.0 - pass_ratio,
                judgment_basis: format!("Coarse scorer: all conditions fail. {}", detail),
                screening_layer: ScreeningLayer::Coarse,
            };
        }

        // ── Fine layer: LLM Judge ─────────────────────────────────
        if let (Some(ref critic), Some(criteria)) = (&self.critic, criteria) {
            match critic.evaluate_trajectory(trajectory, criteria, None).await {
                Ok(critique) => {
                    let confidence = critique.overall_score;
                    let passed = critique.passed;

                    // If confidence is high enough, return fine verdict
                    if confidence >= self.config.fine_min_confidence {
                        let category = detect_category_from_critique(&critique);
                        return ScoringOutput {
                            verdict: if passed { Verdict::Pass } else { Verdict::Fail },
                            score: confidence,
                            problem_category: category,
                            confidence,
                            judgment_basis: format!(
                                "Fine scorer: overall={:.2}, dims={:?}",
                                critique.overall_score, critique.dimension_scores
                            ),
                            screening_layer: ScreeningLayer::Fine,
                        };
                    }

                    // Low confidence → route to human review
                    ScoringOutput {
                        verdict: Verdict::InsufficientInfo,
                        score: confidence,
                        problem_category: detect_category_from_critique(&critique),
                        confidence,
                        judgment_basis: format!(
                            "Low confidence ({:.2}), needs human review. Critique: {:?}",
                            confidence, critique.weaknesses
                        ),
                        screening_layer: ScreeningLayer::HumanReview,
                    }
                }
                Err(e) => ScoringOutput {
                    verdict: Verdict::InsufficientInfo,
                    score: 0.0,
                    problem_category: None,
                    confidence: 0.0,
                    judgment_basis: format!("Fine scorer error: {}", e),
                    screening_layer: ScreeningLayer::HumanReview,
                },
            }
        } else {
            // No Critic configured — return coarse result directly
            ScoringOutput {
                verdict: if all_passed { Verdict::Pass } else { Verdict::Fail },
                score: pass_ratio,
                problem_category: detect_category_from_conditions(&condition_results),
                confidence: pass_ratio,
                judgment_basis: format!("Coarse scorer only (no critic). {}", detail),
                screening_layer: ScreeningLayer::Coarse,
            }
        }
    }
}

/// Detect problem phenomenon from condition check results.
fn detect_category_from_conditions(
    results: &[CheckResult],
) -> Option<ProblemPhenomenon> {
    for r in results {
        if !r.passed {
            let actual_lower = r.actual.to_lowercase();
            if actual_lower.contains("tool") || actual_lower.contains("not called") {
                return Some(ProblemPhenomenon::ToolNotCalled);
            }
            if actual_lower.contains("promise") || actual_lower.contains("承诺") {
                return Some(ProblemPhenomenon::OverPromise);
            }
            if actual_lower.contains("hallucinat") || actual_lower.contains("幻觉") {
                return Some(ProblemPhenomenon::Hallucination);
            }
        }
    }
    None
}

/// Detect problem phenomenon from critique weaknesses.
fn detect_category_from_critique(critique: &Critique) -> Option<ProblemPhenomenon> {
    for w in &critique.weaknesses {
        let wl = w.to_lowercase();
        if wl.contains("promise") || wl.contains("承诺") || wl.contains("over") {
            return Some(ProblemPhenomenon::OverPromise);
        }
        if wl.contains("hallucinat") || wl.contains("幻觉") || wl.contains("编造") {
            return Some(ProblemPhenomenon::Hallucination);
        }
        if wl.contains("factual") || wl.contains("事实") || wl.contains("错误") {
            return Some(ProblemPhenomenon::FactualError);
        }
        if wl.contains("tool") || wl.contains("not call") || wl.contains("未调用") {
            return Some(ProblemPhenomenon::ToolNotCalled);
        }
        if wl.contains("refus") || wl.contains("拒绝") {
            return Some(ProblemPhenomenon::RefusalError);
        }
        if wl.contains("off-topic") || wl.contains("答非所问") {
            return Some(ProblemPhenomenon::NonResponsive);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scorer_config_default() {
        let cfg = ScorerConfig::default();
        assert_eq!(cfg.coarse_pass_threshold, 0.9);
        assert_eq!(cfg.fine_min_confidence, 0.6);
    }
}
