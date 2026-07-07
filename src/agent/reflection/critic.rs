//! LLM-based critic for evaluating and improving agent output.
//!
//! The [`Critic`] wraps an LLM provider and uses carefully crafted prompts
//! to (1) evaluate agent output against quality criteria, and (2) generate
//! improved versions based on the critique.

use std::sync::Arc;

use crate::providers::{CompletionRequest, CompletionResponse, Message, Provider};
use crate::Result;

use super::types::{Critique, QualityCriteria};

// ── Prompt Templates ───────────────────────────────────────────────────────

/// System prompt for the critic evaluation step.
const CRITIC_SYSTEM_PROMPT: &str = r#"You are a quality critic evaluating AI assistant responses.
Rate the output on each criterion from 0.0 (terrible) to 1.0 (perfect).

You MUST respond with ONLY a valid JSON object (no markdown, no backticks):
{
  "dimension_scores": {"criterion_name": 0.85},
  "strengths": ["..."],
  "weaknesses": ["..."],
  "suggested_improvements": ["..."]
}"#;

/// System prompt for the improvement step.
const IMPROVE_SYSTEM_PROMPT: &str = r#"You are an AI assistant improving your previous response based on critique.
Revise your answer to address each weakness and incorporate the suggested improvements.
Output ONLY the improved response text, no additional commentary."#;

// ── Critic ─────────────────────────────────────────────────────────────────

/// LLM-based critic for the Reflection pattern.
///
/// Uses a provider to evaluate agent output against quality criteria and
/// generate improved versions.
#[derive(Clone)]
pub struct Critic {
    /// The LLM provider used for evaluation and improvement.
    provider: Arc<dyn Provider>,
    /// Optional model override for the critic (defaults to provider default).
    model: Option<String>,
}

impl Critic {
    /// Create a new critic with the given provider.
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            model: None,
        }
    }

    /// Set a specific model for the critic to use.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Evaluate agent output against quality criteria.
    ///
    /// Returns a structured [`Critique`] with dimension scores, strengths,
    /// weaknesses, and suggested improvements.
    pub async fn evaluate(
        &self,
        output: &str,
        user_request: &str,
        criteria: &QualityCriteria,
    ) -> Result<Critique> {
        let user_prompt = format!(
            r#"=== USER REQUEST ===
{user_request}

=== ASSISTANT RESPONSE ===
{output}

=== EVALUATION CRITERIA ===
{criteria_text}

Evaluate the response."#,
            criteria_text = criteria.format_for_prompt(),
        );

        let response = self.call_llm(CRITIC_SYSTEM_PROMPT, &user_prompt, Some(2000)).await?;

        let raw = response.message.content.trim().to_string();
        let critique = parse_critique_json(&raw, criteria);

        Ok(critique)
    }

    /// Improve agent output based on a critique.
    ///
    /// Returns the improved response text.
    pub async fn improve(
        &self,
        previous_output: &str,
        critique: &Critique,
        user_request: &str,
    ) -> Result<String> {
        let weaknesses = critique
            .weaknesses
            .iter()
            .map(|w| format!("- {}", w))
            .collect::<Vec<_>>()
            .join("\n");

        let suggestions = critique
            .suggested_improvements
            .iter()
            .map(|s| format!("- {}", s))
            .collect::<Vec<_>>()
            .join("\n");

        let user_prompt = format!(
            r#"=== ORIGINAL REQUEST ===
{user_request}

=== YOUR PREVIOUS RESPONSE ===
{previous_output}

=== CRITIQUE ===
Weaknesses:
{weaknesses}

Suggested improvements:
{suggestions}

=== INSTRUCTIONS ===
Revise your response to address each weakness."#,
        );

        let response = self.call_llm(IMPROVE_SYSTEM_PROMPT, &user_prompt, None).await?;

        let improved = response.message.content.trim().to_string();
        Ok(improved)
    }

    /// Internal helper to call the LLM provider.
    async fn call_llm(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
    ) -> Result<CompletionResponse> {
        let request = CompletionRequest {
            messages: vec![
                Message::system(system),
                Message::user(user),
            ],
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
    }
    .finalize(criteria)
}

/// Default dimension scores (all 0.5 = needs improvement).
fn default_scores() -> std::collections::HashMap<String, f64> {
    let mut map = std::collections::HashMap::new();
    map.insert("Factual Accuracy".to_string(), 0.5);
    map
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

        assert!((critique.dimension_scores.get("Factual Accuracy").copied().unwrap_or(0.0) - 0.9).abs() < 1e-6);
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
        assert!((critique.dimension_scores.get("Factual Accuracy").copied().unwrap_or(0.0) - 0.8).abs() < 1e-6);
    }
}
