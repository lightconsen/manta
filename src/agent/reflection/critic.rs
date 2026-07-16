//! LLM-based critic for trajectory evaluation.
//!
//! The [`Critic`] wraps an LLM provider to evaluate conversation trajectories
//! (multiple turns with tool calls and results) and produce structured
//! critiques with natural-language observations.

use std::sync::Arc;

use super::types::{Critique, QualityCriteria};
use crate::eval::agent_type::AgentType;
use crate::providers::{CompletionRequest, CompletionResponse, Message, Provider};
use crate::Result;

// ── Prompt Template ─────────────────────────────────────────────────────────

/// System prompt for trajectory-level evaluation (retrospect engine).
const TRAJECTORY_CRITIC_PROMPT: &str = r#"You are analyzing a conversation trajectory to identify interaction patterns.

Review the full sequence of turns below — user messages, assistant responses,
tool calls, and tool results — and evaluate. Pay close attention to the actual
content returned by tools (shown in [Tool result: ...]) versus what the
assistant claims in its response.

Evaluation criteria:
1. Evidence faithfulness — does the assistant's response accurately reflect
   the actual tool outputs? Does it make claims not supported by tool results?
   Flag any hallucination or misrepresentation of tool data.
2. Tool usage effectiveness — are tools used appropriately and efficiently?
3. Response quality across turns — are responses consistent and well-structured?
4. Efficiency — are tool calls completing quickly? Are repeated failures or
   timeouts visible? Is the token consumption reasonable for the task?
5. Recurring themes — what patterns repeat across user requests?
6. Improvement opportunities — where could the agent serve the user better?

Output a JSON object with these fields:
{
  "dimension_scores": {"Evidence Faithfulness": 0.9, "Tool Usage": 0.85, "Response Quality": 0.9, "Efficiency": 0.75, "Pattern Recognition": 0.7},
  "strengths": ["...", "..."],
  "weaknesses": ["...", "..."],
  "suggested_improvements": ["...", "..."],
  "observation": "A single-sentence natural-language summary of the key interaction pattern or lesson learned from this window."
}

Important: "observation" should be a concise, actionable insight that would help the agent in future conversations. Write it in English."#;

// ── Critic ─────────────────────────────────────────────────────────────────

/// LLM-based critic for trajectory evaluation.
///
/// Uses a provider to evaluate conversation trajectories and produce
/// structured critiques with pattern observations.
#[derive(Clone)]
pub struct Critic {
    /// The LLM provider used for evaluation.
    provider: Arc<dyn Provider>,
    /// Optional model override for the critic (defaults to provider default).
    model: Option<String>,
}

impl Critic {
    /// Create a new critic with the given provider.
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self { provider, model: None }
    }

    /// Set a specific model for the critic to use.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Evaluate a full conversation trajectory.
    ///
    /// Reviews an interaction window (multiple turns with tool calls and
    /// results) to identify patterns and overall effectiveness.
    ///
    /// `agent_type` optionally provides type-specific scoring emphasis (§02)
    /// that guides the LLM Judge toward the most relevant quality dimensions.
    ///
    /// Returns a [`Critique`] that includes an `observation` field extracted
    /// from the LLM's response for memory persistence.
    pub async fn evaluate_trajectory(
        &self,
        trajectory: &str,
        criteria: &QualityCriteria,
        agent_type: Option<&AgentType>,
    ) -> Result<Critique> {
        // Build system prompt, optionally augmented with agent-type emphasis
        let system_prompt = if let Some(at) = agent_type {
            format!(
                "{}\n\n### Scoring Emphasis\n{}\n\nPay particular attention to the dimensions above when evaluating.",
                TRAJECTORY_CRITIC_PROMPT,
                at.scoring_emphasis(),
            )
        } else {
            TRAJECTORY_CRITIC_PROMPT.to_string()
        };
        let user_prompt = format!(
            r#"{trajectory}

=== EVALUATION CRITERIA ===
{criteria_text}

Evaluate the trajectory above."#,
            criteria_text = criteria.format_for_prompt(),
        );

        let response = self
            .call_llm(&system_prompt, &user_prompt, Some(2000))
            .await?;

        let raw = response.message.content.trim().to_string();
        let mut critique = parse_critique_json(&raw, criteria);

        // Extract the natural-language observation from the LLM output.
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(obs) = parsed.get("observation").and_then(|v| v.as_str()) {
                critique.observation = Some(obs.to_string());
            }
        }

        Ok(critique)
    }

    /// Internal helper to call the LLM provider.
    async fn call_llm(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
    ) -> Result<CompletionResponse> {
        let request = CompletionRequest {
            messages: vec![Message::system(system), Message::user(user)],
            model: self.model.clone(),
            temperature: Some(0.0), // deterministic for evaluation
            max_tokens,
            stream: false,
            ..Default::default()
        };

        self.provider.complete(request).await
    }
}

// ── JSON Parsing ───────────────────────────────────────────────────────────

/// Parse a JSON critique from the LLM response.
///
/// Handles both raw JSON and JSON wrapped in markdown code blocks.
fn parse_critique_json(raw: &str, criteria: &QualityCriteria) -> Critique {
    // Strip markdown code fences if present.
    let cleaned = strip_code_fences(raw);

    // Try to parse as JSON.
    let parsed: serde_json::Value = match serde_json::from_str(cleaned) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Failed to parse critic JSON: {}. Raw: {}", e, raw);
            return default_critique(criteria);
        }
    };

    // Extract dimension_scores.
    let dimension_scores = parsed
        .get("dimension_scores")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0)))
                .collect()
        })
        .unwrap_or_else(default_scores);

    // Extract strengths.
    let strengths = parsed
        .get("strengths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // Extract weaknesses.
    let weaknesses = parsed
        .get("weaknesses")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // Extract suggested improvements.
    let suggested_improvements = parsed
        .get("suggested_improvements")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    Critique {
        dimension_scores,
        strengths,
        weaknesses,
        suggested_improvements,
        overall_score: 0.0,
        passed: false,
        observation: None,
    }
    .finalize(criteria)
}

/// Strip markdown code fences from a string.
fn strip_code_fences(s: &str) -> &str {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("```") {
        // Skip optional language tag on the first line.
        let after_fence = rest.find('\n').map(|pos| &rest[pos + 1..]).unwrap_or(rest);
        // Strip trailing fence.
        if let Some(end) = after_fence.strip_suffix("```") {
            return end.trim();
        }
        return after_fence.trim();
    }
    s
}

/// Fallback critique when JSON parsing fails.
fn default_critique(criteria: &QualityCriteria) -> Critique {
    Critique {
        dimension_scores: default_scores(),
        strengths: vec![],
        weaknesses: vec!["Unable to parse critique".to_string()],
        suggested_improvements: vec![],
        overall_score: 0.5,
        passed: false,
        observation: None,
    }
    .finalize(criteria)
}

/// Default dimension scores (all 0.5 = needs improvement).
fn default_scores() -> std::collections::HashMap<String, f64> {
    let mut map = std::collections::HashMap::new();
    map.insert("Factual Accuracy".to_string(), 0.5);
    map.insert("Efficiency".to_string(), 0.5);
    map
}

/// Compute dynamic memory importance from a critique.
///
/// Low scores in efficiency or tool usage signal costly / problematic
/// patterns that should be remembered more strongly.
pub fn compute_retrospect_importance(critique: &Critique) -> f32 {
    let efficiency_score = critique
        .dimension_scores
        .get("Efficiency")
        .copied()
        .unwrap_or(0.5);
    let tool_usage_score = critique
        .dimension_scores
        .get("Tool Usage")
        .copied()
        .unwrap_or(0.5);
    let weakness_count = critique.weaknesses.len();
    let suggestion_count = critique.suggested_improvements.len();

    let mut importance = 0.5f32;
    importance += (1.0 - efficiency_score as f32) * 0.25;
    importance += (1.0 - tool_usage_score as f32) * 0.25;
    importance += (suggestion_count as f32 * 0.05).min(0.15);
    importance += (weakness_count as f32 * 0.03).min(0.10);
    importance.clamp(0.1, 0.95)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_code_fences_raw_json() {
        let raw = r#"{"dimension_scores": {"Accuracy": 0.9}}"#;
        assert_eq!(strip_code_fences(raw), raw);
    }

    #[test]
    fn test_strip_code_fences_markdown() {
        let raw = "```json\n{\"dimension_scores\": {\"Accuracy\": 0.9}}\n```";
        assert_eq!(strip_code_fences(raw), "{\"dimension_scores\": {\"Accuracy\": 0.9}}");
    }

    #[test]
    fn test_parse_json_critique_valid() {
        let raw = r#"{"dimension_scores":{"Factual Accuracy":0.9,"Completeness":0.7},"strengths":["Good"],"weaknesses":["Missing details"],"suggested_improvements":["Add more detail"]}"#;
        let criteria = QualityCriteria::default();
        let critique = parse_critique_json(raw, &criteria);

        assert!(
            (critique
                .dimension_scores
                .get("Factual Accuracy")
                .copied()
                .unwrap_or(0.0)
                - 0.9)
                .abs()
                < 1e-6
        );
        assert!(critique.strengths.contains(&"Good".to_string()));
        assert!(critique.weaknesses.contains(&"Missing details".to_string()));
    }

    #[test]
    fn test_parse_json_critique_invalid_fallback() {
        let raw = "not valid json at all";
        let criteria = QualityCriteria::default();
        let critique = parse_critique_json(raw, &criteria);
        assert!(!critique.passed);
    }

    #[test]
    fn test_parse_json_critique_with_fences() {
        let raw = "```\n{\"dimension_scores\":{\"Factual Accuracy\":0.8}}\n```";
        let criteria = QualityCriteria::default();
        let critique = parse_critique_json(raw, &criteria);
        assert!(
            (critique
                .dimension_scores
                .get("Factual Accuracy")
                .copied()
                .unwrap_or(0.0)
                - 0.8)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn test_default_scores_includes_efficiency() {
        let scores = default_scores();
        assert!(scores.contains_key("Efficiency"));
        assert!((scores.get("Efficiency").copied().unwrap_or(0.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_compute_retrospect_importance_baseline() {
        let critique = Critique {
            dimension_scores: {
                let mut m = std::collections::HashMap::new();
                m.insert("Efficiency".to_string(), 0.5);
                m.insert("Tool Usage".to_string(), 0.5);
                m
            },
            strengths: vec![],
            weaknesses: vec![],
            suggested_improvements: vec![],
            overall_score: 0.0,
            passed: false,
            observation: None,
        };
        // Baseline 0.5 + (0.5*0.25 + 0.5*0.25) = 0.5 + 0.125 + 0.125 = 0.75
        let importance = compute_retrospect_importance(&critique);
        assert!((importance - 0.75).abs() < 1e-6);
    }

    #[test]
    fn test_compute_retrospect_importance_high_scores_lower_importance() {
        let critique = Critique {
            dimension_scores: {
                let mut m = std::collections::HashMap::new();
                m.insert("Efficiency".to_string(), 0.95);
                m.insert("Tool Usage".to_string(), 0.95);
                m
            },
            strengths: vec!["Good".to_string()],
            weaknesses: vec![],
            suggested_improvements: vec![],
            overall_score: 0.0,
            passed: true,
            observation: None,
        };
        // 0.5 + (0.05*0.25 + 0.05*0.25) = 0.5 + 0.0125 + 0.0125 = 0.525
        let importance = compute_retrospect_importance(&critique);
        assert!((importance - 0.525).abs() < 1e-6);
    }

    #[test]
    fn test_compute_retrospect_importance_low_scores_higher_importance() {
        let critique = Critique {
            dimension_scores: {
                let mut m = std::collections::HashMap::new();
                m.insert("Efficiency".to_string(), 0.1);
                m.insert("Tool Usage".to_string(), 0.1);
                m
            },
            strengths: vec![],
            weaknesses: vec![
                "Slow".to_string(),
                "Errors".to_string(),
                "Verbose".to_string(),
            ],
            suggested_improvements: vec!["Optimize".to_string(), "Retry".to_string()],
            overall_score: 0.0,
            passed: false,
            observation: None,
        };
        // 0.5 + 0.9*0.25 + 0.9*0.25 + 2*0.05 + 3*0.03
        // = 0.5 + 0.225 + 0.225 + 0.10(min 0.15→0.10) + 0.09(min 0.10→0.09)
        // = 0.5 + 0.225 + 0.225 + 0.10 + 0.09 = 1.14 → clamped to 0.95
        let importance = compute_retrospect_importance(&critique);
        assert!((importance - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_compute_retrospect_importance_clamps() {
        let critique = Critique {
            dimension_scores: {
                let mut m = std::collections::HashMap::new();
                m.insert("Efficiency".to_string(), 0.0);
                m.insert("Tool Usage".to_string(), 0.0);
                m
            },
            strengths: vec![],
            weaknesses: vec!["a".to_string(); 10],
            suggested_improvements: vec!["b".to_string(); 10],
            overall_score: 0.0,
            passed: false,
            observation: None,
        };
        // Would be > 0.95 without clamp, should be clamped to 0.95
        let importance = compute_retrospect_importance(&critique);
        assert!(importance <= 0.95);
        assert!(importance >= 0.1);
    }
}
