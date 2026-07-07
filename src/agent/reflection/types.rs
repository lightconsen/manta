//! Core types for the Reflection pattern.
//!
//! Defines what to reflect on, quality criteria, and the structured
//! critique produced by the LLM judge.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// What type of agent output to reflect on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionTarget {
    /// Free-form text response.
    Response,
    /// Code block in a specific language.
    Code {
        /// Optional language identifier (e.g. "python", "rust").
        language: Option<String>,
    },
    /// Execution plan (from GoalPlanner).
    Plan,
    /// Result of a specific tool call.
    ToolResult {
        /// Name of the tool.
        tool_name: String,
    },
}

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
    /// Create criteria with a single threshold for all dimensions.
    pub fn new(dimensions: Vec<QualityDimension>, _default_threshold: f64) -> Self {
        Self {
            dimensions,
            thresholds: HashMap::new(),
        }
    }

    /// Get the threshold for a given dimension label.
    pub fn threshold_for(&self, label: &str) -> f64 {
        self.thresholds.get(label).copied().unwrap_or(0.7)
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
}

impl Critique {
    /// Compute `overall_score` and `passed` from dimension scores and criteria.
    pub fn finalize(mut self, criteria: &QualityCriteria) -> Self {
        let count = self.dimension_scores.len();
        if count > 0 {
            self.overall_score =
                self.dimension_scores.values().sum::<f64>() / count as f64;
        }

        self.passed = self.dimension_scores.iter().all(|(label, score)| {
            *score >= criteria.threshold_for(label)
        });

        self
    }

    /// A critique that immediately passes (used internally).
    pub fn pass() -> Self {
        let mut scores = HashMap::new();
        scores.insert("Factual Accuracy".to_string(), 1.0);
        Self {
            dimension_scores: scores,
            strengths: vec![],
            weaknesses: vec![],
            suggested_improvements: vec![],
            overall_score: 1.0,
            passed: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_criteria_default_dimensions() {
        let c = QualityCriteria::default();
        assert_eq!(c.dimensions.len(), 4);
    }

    #[test]
    fn test_criteria_format() {
        let c = QualityCriteria::default();
        let formatted = c.format_for_prompt();
        assert!(formatted.contains("Factual Accuracy"));
        assert!(formatted.contains("Completeness"));
    }

    #[test]
    fn test_critique_passes_when_all_above_threshold() {
        let criteria = QualityCriteria::default();
        let mut scores = std::collections::HashMap::new();
        scores.insert("Factual Accuracy".to_string(), 0.9);
        scores.insert("Completeness".to_string(), 0.8);

        let critique = Critique {
            dimension_scores: scores,
            strengths: vec![],
            weaknesses: vec![],
            suggested_improvements: vec![],
            overall_score: 0.0,
            passed: false,
        }
        .finalize(&criteria);

        assert!(critique.passed);
        assert!(critique.overall_score > 0.8);
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
        }
        .finalize(&criteria);

        assert!(!critique.passed);
    }

    #[test]
    fn test_reflection_target_serde() {
        let target = ReflectionTarget::Code {
            language: Some("rust".to_string()),
        };
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["code"]["language"], "rust");
    }
}
