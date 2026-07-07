//! Reflection pipeline — self-critique and iterative improvement loop.
//!
//! The [`ReflectionPipeline`] evaluates agent output via an LLM critic,
//! and if the output falls below quality thresholds, iteratively improves
//! it by feeding critique back to the LLM.

use std::sync::Arc;

use tracing::{info, warn};

use crate::providers::{CompletionResponse, Provider};

use super::config::{ReflectionConfig, ReflectionTrigger};
use super::critic::Critic;
use super::types::Critique;

/// Result of a reflection cycle.
#[derive(Debug, Clone)]
pub struct ReflectionResult {
    /// The final (possibly improved) response content.
    pub final_content: String,
    /// Number of improvement iterations performed.
    pub iterations: usize,
    /// Full critique history across all iterations.
    pub critique_history: Vec<Critique>,
}

impl ReflectionResult {
    /// Format the critique history into a concise lesson summary for memory.
    ///
    /// Extracts weaknesses and improvements from the last critique that
    /// triggered an improvement, producing a single-paragraph lesson.
    pub fn format_lesson(&self) -> String {
        let last_critique = match self.critique_history.last() {
            Some(c) => c,
            None => return String::new(),
        };

        // If the last critique passed, use the previous iteration's data.
        let relevant = if last_critique.passed && self.critique_history.len() > 1 {
            &self.critique_history[self.critique_history.len() - 2]
        } else {
            last_critique
        };

        let mut parts: Vec<String> = Vec::new();

        if !relevant.weaknesses.is_empty() {
            let w = relevant.weaknesses.join("; ");
            parts.push(format!("缺点: {}", w));
        }

        if !relevant.suggested_improvements.is_empty() {
            let s = relevant.suggested_improvements.join("; ");
            parts.push(format!("改进方向: {}", s));
        }

        if parts.is_empty() {
            return String::new();
        }

        format!(
            "Reflection 发现回复中存在问题，经 {} 轮迭代后改进。{}",
            self.iterations,
            parts.join("。")
        )
    }

    /// Compute an importance score (0.0–1.0) for memory persistence.
    ///
    /// Uses the inverse of the first iteration's overall score so that
    /// larger gaps (worse initial output) yield higher importance.
    pub fn importance(&self) -> f32 {
        let first_score = self
            .critique_history
            .first()
            .map(|c| c.overall_score)
            .unwrap_or(0.5);

        // Map 0.0→1.0 → importance 1.0→0.3 (worse output = more important)
        let raw = 1.0 - first_score;
        (raw.max(0.0).min(1.0) * 0.7 + 0.3) as f32
    }
}

/// The reflection pipeline that drives self-critique and improvement.
#[derive(Clone)]
pub struct ReflectionPipeline {
    /// Configuration for when and how to reflect.
    pub config: ReflectionConfig,
    /// The LLM critic used for evaluation and improvement.
    critic: Critic,
}

impl ReflectionPipeline {
    /// Create a new reflection pipeline with the given config and provider.
    pub fn new(config: ReflectionConfig, provider: Arc<dyn Provider>) -> Self {
        let mut critic = Critic::new(provider);
        if let Some(ref model) = config.critic_model {
            critic = critic.with_model(model.clone());
        }

        Self { config, critic }
    }

    /// Determine whether reflection should be triggered for this response.
    pub fn should_trigger(
        &self,
        _user_message: &str,
        response: &CompletionResponse,
    ) -> bool {
        match &self.config.trigger {
            ReflectionTrigger::Always => true,
            ReflectionTrigger::AfterToolCall => response.message.tool_calls.is_some(),
            ReflectionTrigger::OnCodeGeneration => {
                response.message.content.contains("```")
            }
            ReflectionTrigger::Adaptive { min_tokens } => {
                response.message.content.len() > *min_tokens
                    || response.message.tool_calls.is_some()
            }
        }
    }

    /// Run the full reflection cycle on agent output.
    ///
    /// 1. Evaluate the output using the LLM critic.
    /// 2. If it passes all quality thresholds, return immediately.
    /// 3. Otherwise, generate an improved version and repeat.
    pub async fn reflect(
        &self,
        content: &str,
        user_request: &str,
        _tool_results: &[String],
    ) -> ReflectionResult {
        let mut current = content.to_string();
        let mut critique_history = Vec::new();
        let criteria = &self.config.criteria;

        for iteration in 0..self.config.max_iterations {
            // 1. Evaluate current output.
            let critique = match self
                .critic
                .evaluate(&current, user_request, criteria)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    warn!("Reflection critic evaluation failed at iteration {}: {}", iteration, e);
                    // Continue with default critique to avoid blocking the response.
                    let mut fallback = Critique::pass();
                    fallback.weaknesses =
                        vec![format!("Critic evaluation failed: {}", e)];
                    critique_history.push(fallback);
                    break;
                }
            };

            let passed = critique.passed;
            critique_history.push(critique.clone());

            if passed {
                info!(
                    "Reflection passed on iteration {} with score {:.2}",
                    iteration,
                    critique.overall_score
                );
                break;
            }

            info!(
                "Reflection iteration {}: score {:.2}, improving...",
                iteration,
                critique.overall_score
            );

            if iteration + 1 >= self.config.max_iterations {
                info!(
                    "Reflection reached max iterations ({}), returning best effort",
                    self.config.max_iterations
                );
                break;
            }

            // 2. Generate improved version based on critique.
            match self
                .critic
                .improve(&current, &critique, user_request)
                .await
            {
                Ok(improved) => {
                    current = improved;
                }
                Err(e) => {
                    warn!("Reflection improvement failed at iteration {}: {}", iteration, e);
                    break;
                }
            }
        }

        let iterations = critique_history.len();
        ReflectionResult {
            final_content: current,
            iterations,
            critique_history,
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::mock::MockProvider;

    fn mock_provider() -> Arc<dyn Provider> {
        Arc::new(MockProvider::new())
    }

    #[test]
    fn test_trigger_adaptive_short() {
        let config = ReflectionConfig {
            trigger: ReflectionTrigger::Adaptive { min_tokens: 200 },
            ..Default::default()
        };
        let pipeline = ReflectionPipeline::new(config, mock_provider());
        let resp = CompletionResponse {
            message: crate::providers::Message::assistant("hi"),
            usage: None,
            model: "test".to_string(),
            finish_reason: None,
        };
        assert!(!pipeline.should_trigger("hello", &resp));
    }

    #[test]
    fn test_trigger_adaptive_long() {
        let config = ReflectionConfig {
            trigger: ReflectionTrigger::Adaptive { min_tokens: 10 },
            ..Default::default()
        };
        let pipeline = ReflectionPipeline::new(config, mock_provider());
        let resp = CompletionResponse {
            message: crate::providers::Message::assistant(
                "this is a sufficiently long response to trigger reflection",
            ),
            usage: None,
            model: "test".to_string(),
            finish_reason: None,
        };
        assert!(pipeline.should_trigger("hello", &resp));
    }

    #[test]
    fn test_trigger_code_generation() {
        let config = ReflectionConfig {
            trigger: ReflectionTrigger::OnCodeGeneration,
            ..Default::default()
        };
        let pipeline = ReflectionPipeline::new(config, mock_provider());
        let resp = CompletionResponse {
            message: crate::providers::Message::assistant(
                "Here is the code:\n```rust\nfn main() {}\n```",
            ),
            usage: None,
            model: "test".to_string(),
            finish_reason: None,
        };
        assert!(pipeline.should_trigger("write code", &resp));
    }

    #[test]
    fn test_trigger_always() {
        let config = ReflectionConfig {
            trigger: ReflectionTrigger::Always,
            ..Default::default()
        };
        let pipeline = ReflectionPipeline::new(config, mock_provider());
        let resp = CompletionResponse {
            message: crate::providers::Message::assistant("ok"),
            usage: None,
            model: "test".to_string(),
            finish_reason: None,
        };
        assert!(pipeline.should_trigger("hi", &resp));
    }

    #[tokio::test]
    async fn test_reflect_with_mock_provider() {
        let config = ReflectionConfig {
            max_iterations: 1,
            ..Default::default()
        };
        let pipeline = ReflectionPipeline::new(config, mock_provider());
        let result = pipeline.reflect("test output", "test request", &[]).await;
        // Mock provider always returns fixed text, so it'll get evaluated
        // and the loop runs at least once.
        assert_eq!(result.final_content, "test output");
    }
}
