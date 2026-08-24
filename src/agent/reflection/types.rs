//! Core types for trajectory reflection.
//!
//! Defines quality criteria and the structured critique produced by the
//! LLM judge during trajectory evaluation.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A dimension of quality used during evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityDimension {
    /// Factual correctness and absence of hallucinations.
    FactualAccuracy,
    /// Whether the response fully addresses the request.
    Completeness,
    /// Internal consistency — no contradictions.
    Consistency,
    /// How clear and well-structured the response is.
    Clarity,
    /// Whether the response provides actionable information.
    Actionable,
    /// Safety — no harmful, toxic, or dangerous content.
    Safety,
    /// How well the response follows given instructions.
    InstructionFollowing,
    /// Context retention and anaphora resolution across turns (§03).
    ContextRetention,
    /// Goal switching adaptability when user changes topic (§03).
    GoalSwitch,
    /// Emotion & sentiment handling in responses (§03).
    EmotionHandling,
    /// Evidence consistency — response claims match tool result evidence (§06).
    EvidenceConsistency,
    /// A custom dimension with a user-provided name.
    Custom(String),
}

impl QualityDimension {
    /// Human-readable label for display in prompts.
    pub fn label(&self) -> &str {
        match self {
            Self::FactualAccuracy => "Factual Accuracy",
            Self::Completeness => "Completeness",
            Self::Consistency => "Consistency",
            Self::Clarity => "Clarity",
            Self::Actionable => "Actionable",
            Self::Safety => "Safety",
            Self::InstructionFollowing => "Instruction Following",
            Self::ContextRetention => "Context Retention",
            Self::GoalSwitch => "Goal Switch",
            Self::EmotionHandling => "Emotion Handling",
            Self::EvidenceConsistency => "Evidence Consistency",
            Self::Custom(name) => name.as_str(),
        }
    }
}

/// Quality criteria for evaluating agent output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCriteria {
    /// Dimensions to evaluate.
    pub dimensions: Vec<QualityDimension>,
    /// Per-dimension score thresholds (0.0–1.0) required to pass.
    /// Dimensions not listed here default to `pass_threshold`.
    pub thresholds: HashMap<String, f64>,
}

impl QualityCriteria {
    /// Create criteria with a default threshold for all dimensions.
    /// Individual dimensions can be overridden via
    /// [`thresholds`](Self::thresholds).
    pub fn new(dimensions: Vec<QualityDimension>, default_threshold: f64) -> Self {
        let mut thresholds = HashMap::new();
        for dim in &dimensions {
            thresholds.insert(dim.label().to_string(), default_threshold);
        }
        Self { dimensions, thresholds }
    }

    /// Get the threshold for a given dimension label.
    ///
    /// Label matching is normalized (case-insensitive, `_`/space-insensitive)
    /// so YAML keys like `factual_accuracy` match the label "Factual Accuracy".
    pub fn threshold_for(&self, label: &str) -> f64 {
        let want = normalize_label(label);
        self.thresholds
            .iter()
            .find(|(k, _)| normalize_label(k) == want)
            .map(|(_, v)| *v)
            .unwrap_or(0.7)
    }

    /// Format dimensions for inclusion in a prompt.
    pub fn format_for_prompt(&self) -> String {
        self.dimensions
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let label = d.label();
                let threshold = self.threshold_for(label);
                format!("{}. {} (threshold: {:.1})", i + 1, label, threshold)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for QualityCriteria {
    fn default() -> Self {
        Self {
            dimensions: vec![
                QualityDimension::FactualAccuracy,
                QualityDimension::Completeness,
                QualityDimension::Clarity,
                QualityDimension::InstructionFollowing,
                QualityDimension::EvidenceConsistency,
            ],
            thresholds: HashMap::new(),
        }
    }
}

/// Structured critique produced by the LLM judge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Critique {
    /// Score for each dimension (label → score).
    pub dimension_scores: HashMap<String, f64>,
    /// What the response does well.
    pub strengths: Vec<String>,
    /// What the response does poorly.
    pub weaknesses: Vec<String>,
    /// Concrete suggestions for improvement.
    pub suggested_improvements: Vec<String>,
    /// Overall weighted score (0.0–1.0).
    #[serde(default)]
    pub overall_score: f64,
    /// Whether this critique meets all thresholds.
    #[serde(default)]
    pub passed: bool,
    /// Natural-language observation from trajectory evaluation.
    /// Populated by [`Critic::evaluate_trajectory`].
    #[serde(default, skip)]
    pub observation: Option<String>,
}

impl Critique {
    /// Compute `overall_score` and `passed` from dimension scores and criteria.
    ///
    /// Gating uses ONLY the dimensions the task declared: extra dimensions the
    /// judge volunteers (e.g. "Pattern Recognition" from the prompt template)
    /// are informational and must not fail the critique. A declared dimension
    /// the judge did not score fails closed with an explanatory weakness.
    pub fn finalize(mut self, criteria: &QualityCriteria) -> Self {
        let count = self.dimension_scores.len();
        if count > 0 {
            self.overall_score = self.dimension_scores.values().sum::<f64>() / count as f64;
        }

        let mut passed = true;
        for dim in &criteria.dimensions {
            let label = dim.label();
            let want = normalize_label(label);
            let score = self
                .dimension_scores
                .iter()
                .find(|(k, _)| normalize_label(k) == want)
                .map(|(_, v)| *v);
            match score {
                Some(s) if s >= criteria.threshold_for(label) => {}
                Some(_) => passed = false,
                None => {
                    passed = false;
                    self.weaknesses
                        .push(format!("Judge did not score declared dimension '{label}'"));
                }
            }
        }
        self.passed = passed;
        self
    }
}

/// Normalize a dimension label for comparison: lowercase, ignoring spaces
/// and underscores, so "Factual Accuracy", "factual_accuracy" and
/// "factualAccuracy" all compare equal.
fn normalize_label(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '_' && !c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_criteria_default_dimensions() {
        let c = QualityCriteria::default();
        assert_eq!(c.dimensions.len(), 5);
    }

    #[test]
    fn test_criteria_format() {
        let c = QualityCriteria::default();
        let formatted = c.format_for_prompt();
        assert!(formatted.contains("Factual Accuracy"));
        assert!(formatted.contains("Completeness"));
        assert!(formatted.contains("Evidence Consistency"));
    }

    #[test]
    fn test_critique_passes_when_all_above_threshold() {
        let criteria = QualityCriteria::default();
        let mut scores = std::collections::HashMap::new();
        scores.insert("Factual Accuracy".to_string(), 0.9);
        scores.insert("Completeness".to_string(), 0.8);
        scores.insert("Clarity".to_string(), 0.85);
        scores.insert("Instruction Following".to_string(), 0.9);
        scores.insert("Evidence Consistency".to_string(), 0.8);
        // Extra judge-volunteered dimensions are informational only and must
        // not gate, even when below the default threshold.
        scores.insert("Pattern Recognition".to_string(), 0.5);

        let critique = Critique {
            dimension_scores: scores,
            strengths: vec![],
            weaknesses: vec![],
            suggested_improvements: vec![],
            overall_score: 0.0,
            passed: false,
            observation: None,
        }
        .finalize(&criteria);

        assert!(critique.passed);
        assert!(critique.overall_score > 0.7);
    }

    #[test]
    fn test_critique_fails_when_declared_dimension_unscored() {
        let criteria = QualityCriteria::default();
        let mut scores = std::collections::HashMap::new();
        scores.insert("Factual Accuracy".to_string(), 0.9);

        let critique = Critique {
            dimension_scores: scores,
            strengths: vec![],
            weaknesses: vec![],
            suggested_improvements: vec![],
            overall_score: 0.0,
            passed: false,
            observation: None,
        }
        .finalize(&criteria);

        assert!(!critique.passed);
        assert!(critique
            .weaknesses
            .iter()
            .any(|w| w.contains("did not score declared dimension")));
    }

    #[test]
    fn test_threshold_for_matches_yaml_snake_case_keys() {
        // YAML thresholds use snake_case ("factual_accuracy"); labels are
        // title case ("Factual Accuracy"). Lookup must normalize.
        let mut thresholds = std::collections::HashMap::new();
        thresholds.insert("factual_accuracy".to_string(), 0.9);
        let criteria = QualityCriteria {
            dimensions: vec![QualityDimension::FactualAccuracy],
            thresholds,
        };
        assert!((criteria.threshold_for("Factual Accuracy") - 0.9).abs() < 1e-9);
    }

    #[test]
    fn test_critique_fails_when_below_threshold() {
        let criteria = QualityCriteria::default();
        let mut scores = std::collections::HashMap::new();
        scores.insert("Factual Accuracy".to_string(), 0.3);

        let critique = Critique {
            dimension_scores: scores,
            strengths: vec![],
            weaknesses: vec![],
            suggested_improvements: vec![],
            overall_score: 0.0,
            passed: false,
            observation: None,
        }
        .finalize(&criteria);

        assert!(!critique.passed);
    }
}
