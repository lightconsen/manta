//! Configuration for the Reflection pattern.
//!
//! Controls when reflection is triggered, what quality criteria to use,
//! and how many improvement iterations to attempt.

use serde::{Deserialize, Serialize};

use super::types::QualityCriteria;

/// When to trigger reflection on agent output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReflectionTrigger {
    /// Always reflect on every response.
    Always,
    /// Only when the response includes tool calls.
    AfterToolCall,
    /// Only when the response contains code blocks.
    OnCodeGeneration,
    /// Adaptive trigger — reflects when output is long enough to benefit.
    Adaptive {
        /// Minimum response length (in characters) to trigger reflection.
        #[serde(default = "default_min_tokens")]
        min_tokens: usize,
    },
}

const fn default_min_tokens() -> usize {
    200
}

impl Default for ReflectionTrigger {
    fn default() -> Self {
        Self::Adaptive { min_tokens: 200 }
    }
}

/// Configuration for the general-purpose Reflection pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    /// When to trigger reflection.
    #[serde(default)]
    pub trigger: ReflectionTrigger,
    /// Quality criteria for evaluating responses.
    #[serde(default)]
    pub criteria: QualityCriteria,
    /// Maximum number of evaluate–improve cycles.
    ///
    /// Each iteration runs one evaluation and (if the response doesn't pass)
    /// one improvement. The total number of LLM calls is at most
    /// `max_iterations` evaluations + `max_iterations - 1` improvements.
    ///
    /// Setting to `1` means "evaluate once, never improve" — useful for
    /// measurement-only mode. Default: 3.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    /// Minimum overall score (0.0–1.0) required to pass (default: 0.7).
    #[serde(default = "default_pass_threshold")]
    pub pass_threshold: f64,
    /// Optional model to use for the critic.
    /// When `None`, the agent's own provider/model is used.
    #[serde(default)]
    pub critic_model: Option<String>,
}

const fn default_max_iterations() -> usize {
    3
}

fn default_pass_threshold() -> f64 {
    0.7
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            trigger: ReflectionTrigger::default(),
            criteria: QualityCriteria::default(),
            max_iterations: default_max_iterations(),
            pass_threshold: default_pass_threshold(),
            critic_model: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ReflectionConfig::default();
        assert_eq!(config.max_iterations, 3);
        assert!((config.pass_threshold - 0.7).abs() < 1e-6);
        assert!(matches!(config.trigger, ReflectionTrigger::Adaptive { .. }));
    }

    #[test]
    fn test_trigger_serde_roundtrip() {
        let trigger = ReflectionTrigger::Adaptive { min_tokens: 500 };
        let json = serde_json::to_value(&trigger).unwrap();
        let deserialized: ReflectionTrigger = serde_json::from_value(json).unwrap();
        assert!(matches!(deserialized, ReflectionTrigger::Adaptive { min_tokens: 500 }));
    }
}
