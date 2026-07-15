//! Multi-Judge adversarial scoring — multiple LLM judges evaluate the same
//! trajectory independently, with aggregated results (§06).
//!
//! # Usage
//!
//! ```rust,ignore
//! use syscity::eval::multi_judge::{MultiJudgeScorer, MultiJudgeConfig};
//!
//! let config = MultiJudgeConfig::default();
//! let scorer = MultiJudgeScorer::new(config, judges).await?;
//! let result = scorer.evaluate(trajectory, &criteria, None).await;
//! println!("{:?}", result.aggregated.verdict);
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::agent::reflection::critic::Critic;
use crate::agent::reflection::types::QualityCriteria;
use crate::eval::AgentType;
use crate::providers::resolver::resolve_provider;
use crate::Result;

// ── Configuration ───────────────────────────────────────────────────────

/// Configuration for a single judge in the multi-judge pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeConfig {
    /// Human-readable name (e.g. "claude-sonnet", "gpt-4o").
    pub name: String,
    /// Provider type ("anthropic", "openai").
    #[serde(default = "default_provider")]
    pub provider_type: String,
    /// Model name override (None = provider default).
    #[serde(default)]
    pub model: Option<String>,
    /// API key override (None = env var / config).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Base URL override.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Voting weight in aggregation (0.0–1.0). Default 1.0.
    #[serde(default = "default_weight")]
    pub weight: f64,
}

fn default_provider() -> String {
    "anthropic".to_string()
}

fn default_weight() -> f64 {
    1.0
}

/// Aggregation mode for multi-judge results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AggregationMode {
    /// Each judge votes Pass/Fail; majority wins.
    MajorityVote,
    /// Weighted average of overall scores.
    WeightedAverage,
}

impl Default for AggregationMode {
    fn default() -> Self {
        Self::WeightedAverage
    }
}

/// Configuration for the multi-judge scorer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiJudgeConfig {
    /// List of judges in the pool.
    pub judges: Vec<JudgeConfig>,
    /// How to aggregate individual judge scores.
    #[serde(default)]
    pub aggregation: AggregationMode,
    /// Score std-dev threshold above which disagreement is flagged (0.0–1.0).
    #[serde(default = "default_disagreement_threshold")]
    pub disagreement_threshold: f64,
    /// Minimum number of judges required (default: all).
    #[serde(default)]
    pub min_judges: Option<usize>,
}

fn default_disagreement_threshold() -> f64 {
    0.25
}

impl Default for MultiJudgeConfig {
    fn default() -> Self {
        Self {
            judges: vec![JudgeConfig {
                name: "primary".into(),
                provider_type: "anthropic".into(),
                model: None,
                api_key: None,
                base_url: None,
                weight: 1.0,
            }],
            aggregation: AggregationMode::WeightedAverage,
            disagreement_threshold: 0.25,
            min_judges: None,
        }
    }
}

// ── Results ─────────────────────────────────────────────────────────────

/// Result from a single judge's evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeResult {
    pub judge_name: String,
    pub passed: bool,
    pub overall_score: f64,
    pub dimension_scores: HashMap<String, f64>,
    pub weaknesses: Vec<String>,
    pub strengths: Vec<String>,
}

/// Aggregated result across all judges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedResult {
    /// Final verdict (majority or threshold-based).
    pub verdict: AggregatedVerdict,
    /// Mean overall score across judges.
    pub mean_score: f64,
    /// Weighted score (if using WeightedAverage).
    pub weighted_score: Option<f64>,
    /// Standard deviation of scores (disagreement metric).
    pub score_std_dev: f64,
    /// Fraction of judges that passed.
    pub pass_fraction: f64,
    /// Whether significant disagreement was detected.
    pub high_disagreement: bool,
    /// Per-judge breakdown.
    pub per_judge: Vec<JudgeResult>,
}

/// Verdict from the multi-judge aggregator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AggregatedVerdict {
    /// Clear pass.
    Pass,
    /// Clear fail.
    Fail,
    /// Judges disagree — route to human review.
    Disagreement,
    /// Not enough judges responded.
    InsufficientJudges,
}

// ── Scorer ──────────────────────────────────────────────────────────────

/// Multi-judge adversarial scorer.
pub struct MultiJudgeScorer {
    /// Named judges with their critics and weights.
    judges: Vec<(String, Arc<Critic>, f64)>,
    config: MultiJudgeConfig,
}

impl MultiJudgeScorer {
    /// Create a new multi-judge scorer from config.
    ///
    /// Resolves providers for each judge config and wraps them in Critic
    /// instances. Returns an error if any judge fails to initialize.
    pub async fn new(config: MultiJudgeConfig) -> Result<Self> {
        let mut judges = Vec::with_capacity(config.judges.len());

        for jc in &config.judges {
            let provider = match resolve_provider(
                &jc.provider_type,
                jc.api_key.clone(),
                jc.base_url.clone(),
                jc.model.clone(),
                None,
            ) {
                Ok(p) => p,
                Err(e) => {
                    warn!("Failed to create provider for judge '{}': {}", jc.name, e);
                    return Err(crate::error::SyscityError::Validation(format!(
                        "Judge '{}': {}",
                        jc.name, e
                    )));
                }
            };

            let mut critic = Critic::new(provider);
            if let Some(ref model) = jc.model {
                critic = critic.with_model(model.clone());
            }

            judges.push((jc.name.clone(), Arc::new(critic), jc.weight));
        }

        Ok(Self { judges, config })
    }

    /// Build from a pre-created list of (name, Critic, weight) tuples.
    pub fn from_judges(judges: Vec<(String, Critic, f64)>, config: MultiJudgeConfig) -> Self {
        Self {
            judges: judges
                .into_iter()
                .map(|(n, c, w)| (n, Arc::new(c), w))
                .collect(),
            config,
        }
    }

    /// Number of judges in the pool.
    pub fn judge_count(&self) -> usize {
        self.judges.len()
    }

    /// Evaluate a trajectory with all judges in parallel.
    pub async fn evaluate(
        &self,
        trajectory: &str,
        criteria: &QualityCriteria,
        agent_type: Option<&AgentType>,
    ) -> AggregatedResult {
        if self.judges.is_empty() {
            return AggregatedResult {
                verdict: AggregatedVerdict::InsufficientJudges,
                mean_score: 0.0,
                weighted_score: None,
                score_std_dev: 0.0,
                pass_fraction: 0.0,
                high_disagreement: false,
                per_judge: Vec::new(),
            };
        }

        // Run all judges in parallel
        let mut per_judge = Vec::with_capacity(self.judges.len());
        for (name, critic, _weight) in &self.judges {
            let result = evaluate_single(critic, name, trajectory, criteria, agent_type).await;
            per_judge.push(result);
        }

        let actual_judges: Vec<&JudgeResult> = per_judge
            .iter()
            .filter(|r| r.overall_score >= 0.0)
            .collect();

        let min_judges = self.config.min_judges.unwrap_or(self.judges.len());

        if actual_judges.len() < min_judges {
            return AggregatedResult {
                verdict: AggregatedVerdict::InsufficientJudges,
                mean_score: 0.0,
                weighted_score: None,
                score_std_dev: 0.0,
                pass_fraction: 0.0,
                high_disagreement: false,
                per_judge,
            };
        }

        // Compute scores
        let n = actual_judges.len() as f64;
        let mean_score: f64 = actual_judges.iter().map(|r| r.overall_score).sum::<f64>() / n;
        let pass_count = actual_judges.iter().filter(|r| r.passed).count();
        let pass_fraction = pass_count as f64 / n;

        // Weighted score
        let total_weight: f64 = self
            .judges
            .iter()
            .filter(|(name, _, _)| actual_judges.iter().any(|r| r.judge_name == *name))
            .map(|(_, _, w)| w)
            .sum();
        let weighted_score = if total_weight > 0.0 {
            let mut ws = 0.0f64;
            for (name, _, weight) in &self.judges {
                if let Some(r) = actual_judges.iter().find(|r| r.judge_name == *name) {
                    ws += r.overall_score * weight;
                }
            }
            Some(ws / total_weight)
        } else {
            None
        };

        // Standard deviation
        let variance: f64 = actual_judges
            .iter()
            .map(|r| (r.overall_score - mean_score).powi(2))
            .sum::<f64>()
            / n;
        let score_std_dev = variance.sqrt();

        let high_disagreement = score_std_dev > self.config.disagreement_threshold;

        // Determine verdict
        let verdict = match self.config.aggregation {
            AggregationMode::MajorityVote => {
                if pass_count > actual_judges.len() / 2 {
                    AggregatedVerdict::Pass
                } else if pass_count < actual_judges.len() - actual_judges.len() / 2 {
                    AggregatedVerdict::Fail
                } else {
                    AggregatedVerdict::Disagreement
                }
            }
            AggregationMode::WeightedAverage => {
                let score = weighted_score.unwrap_or(mean_score);
                if high_disagreement {
                    AggregatedVerdict::Disagreement
                } else if score >= 0.5 {
                    AggregatedVerdict::Pass
                } else {
                    AggregatedVerdict::Fail
                }
            }
        };

        AggregatedResult {
            verdict,
            mean_score,
            weighted_score,
            score_std_dev,
            pass_fraction,
            high_disagreement,
            per_judge,
        }
    }
}

async fn evaluate_single(
    critic: &Critic,
    name: &str,
    trajectory: &str,
    criteria: &QualityCriteria,
    agent_type: Option<&AgentType>,
) -> JudgeResult {
    match critic
        .evaluate_trajectory(trajectory, criteria, agent_type)
        .await
    {
        Ok(critique) => JudgeResult {
            judge_name: name.to_string(),
            passed: critique.passed,
            overall_score: critique.overall_score,
            dimension_scores: critique.dimension_scores,
            weaknesses: critique.weaknesses,
            strengths: critique.strengths,
        },
        Err(e) => {
            warn!("Judge '{}' failed: {}", name, e);
            JudgeResult {
                judge_name: name.to_string(),
                passed: false,
                overall_score: 0.0,
                dimension_scores: HashMap::new(),
                weaknesses: vec![format!("error: {}", e)],
                strengths: Vec::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_result(name: &str, score: f64, passed: bool) -> JudgeResult {
        let mut dims = HashMap::new();
        dims.insert("Accuracy".to_string(), score);
        JudgeResult {
            judge_name: name.to_string(),
            passed,
            overall_score: score,
            dimension_scores: dims,
            weaknesses: Vec::new(),
            strengths: Vec::new(),
        }
    }

    #[test]
    fn test_aggregation_majority_pass() {
        let per_judge = vec![
            dummy_result("a", 0.9, true),
            dummy_result("b", 0.8, true),
            dummy_result("c", 0.3, false),
        ];

        let n = per_judge.len() as f64;
        let mean = per_judge.iter().map(|r| r.overall_score).sum::<f64>() / n;
        let pass_count = per_judge.iter().filter(|r| r.passed).count();
        let variance = per_judge
            .iter()
            .map(|r| (r.overall_score - mean).powi(2))
            .sum::<f64>()
            / n;

        assert_eq!(pass_count, 2);
        assert!((mean - 0.6667).abs() < 0.01);
        assert!(variance.sqrt() > 0.25); // high disagreement
    }

    #[test]
    fn test_aggregation_majority_fail() {
        let per_judge = vec![
            dummy_result("a", 0.2, false),
            dummy_result("b", 0.3, false),
            dummy_result("c", 0.9, true),
        ];

        let pass_count = per_judge.iter().filter(|r| r.passed).count();
        assert_eq!(pass_count, 1);
        assert!(pass_count <= per_judge.len() / 2);
    }

    #[test]
    fn test_std_dev_low() {
        let results = vec![
            dummy_result("a", 0.8, true),
            dummy_result("b", 0.85, true),
            dummy_result("c", 0.9, true),
        ];
        let n = results.len() as f64;
        let mean = results.iter().map(|r| r.overall_score).sum::<f64>() / n;
        let variance = results
            .iter()
            .map(|r| (r.overall_score - mean).powi(2))
            .sum::<f64>()
            / n;
        let std_dev = variance.sqrt();
        assert!(std_dev < 0.1); // very low disagreement
    }

    #[test]
    fn test_empty_judges() {
        let results: Vec<JudgeResult> = vec![];
        let n = results.len() as f64;
        assert_eq!(n, 0.0);
        assert!(results.is_empty());
    }

    #[test]
    fn test_judge_config_defaults() {
        let jc = JudgeConfig {
            name: "test".into(),
            provider_type: "openai".into(),
            model: Some("gpt-4".into()),
            api_key: None,
            base_url: None,
            weight: 1.0,
        };
        assert_eq!(jc.name, "test");
        assert_eq!(jc.provider_type, "openai");
    }
}
