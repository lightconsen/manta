//! Retrospect engine — periodic, background trajectory reflection.
//!
//! The retrospect engine runs as a background task every N turns, reviewing the
//! last M turns of conversation and writing interaction patterns to memory.
//! Unlike synchronous per-response refinement approaches,
//! it never blocks the response and never modifies output content.

use std::sync::Arc;

use super::config::RetrospectConfig;
use super::critic::Critic;
use super::trajectory::{Trajectory, TrajectoryStep, TrajectoryWindow};
use super::types::{Critique, QualityCriteria};
use crate::agent::turns::Turn;
use crate::providers::Provider;
use crate::Result;

/// Result of a retrospect cycle — trajectory critique + observation for memory.
#[derive(Debug, Clone)]
pub struct RetrospectResult {
    /// The structured critique of the conversation window.
    pub critique: Critique,
    /// Natural-language observation extracted from the critique.
    /// This is the key output persisted to memory.
    pub observation: String,
    /// Turn count at which this retrospect fired.
    pub turn_count: usize,
}

/// Periodic trajectory reflection engine.
///
/// Runs every [`RetrospectConfig::interval`] turns, reviewing the last
/// [`RetrospectConfig::window_size`] turns to identify interaction patterns.
#[derive(Clone)]
pub struct RetrospectEngine {
    /// Retrospect scheduling and window configuration.
    pub config: RetrospectConfig,
    /// The LLM critic used for trajectory evaluation.
    critic: Critic,
}

impl RetrospectEngine {
    /// Create a new retrospect engine with the given config and provider.
    pub fn new(config: RetrospectConfig, provider: Arc<dyn Provider>) -> Self {
        Self {
            critic: Critic::new(provider),
            config,
        }
    }

    /// Set a specific model for the critic to use.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.critic = self.critic.with_model(model);
        self
    }

    /// Build a [`Trajectory`] from the last N turns.
    ///
    /// Each turn's `user_message`, `assistant_response`, `tool_calls`, and
    /// `token_usage` are mapped to [`TrajectoryStep`] entries. Turns beyond the
    /// configured window size are discarded.
    pub fn build_trajectory(&self, turns: &[Turn], total_turns: usize) -> Trajectory {
        let window = self.config.window_size.min(turns.len());
        let recent = &turns[turns.len().saturating_sub(window)..];

        let trajectory_turns: Vec<TrajectoryWindow> = recent
            .iter()
            .map(|turn| {
                let mut steps = Vec::new();

                // Tool calls and their results
                for record in &turn.tool_calls {
                    steps.push(TrajectoryStep::ToolCall {
                        name: record.name.clone(),
                        args: record.args.clone(),
                        duration_ms: record.duration_ms,
                    });
                    steps.push(TrajectoryStep::ToolResult {
                        name: record.name.clone(),
                        content: record.result.clone(),
                        success: record.success,
                    });
                }

                if !turn.assistant_response.is_empty() {
                    steps.push(TrajectoryStep::AssistantResponse {
                        content: turn.assistant_response.clone(),
                    });
                }

                // Token usage
                if let Some(usage) = &turn.token_usage {
                    steps.push(TrajectoryStep::TokenUsage {
                        prompt_tokens: usage.prompt_tokens,
                        completion_tokens: usage.completion_tokens,
                        total_tokens: usage.total_tokens,
                    });
                }

                TrajectoryWindow {
                    index: turn.index,
                    user_message: turn.user_message.clone(),
                    steps,
                }
            })
            .collect();

        Trajectory {
            turns: trajectory_turns,
            total_turns,
            window_size: window,
        }
    }

    /// Run a full trajectory reflection cycle.
    ///
    /// 1. Builds a [`Trajectory`] from the provided turns.
    /// 2. Formats it and sends to the LLM critic for evaluation.
    /// 3. Extracts the natural-language observation.
    ///
    /// Returns a [`RetrospectResult`] with the critique and observation.
    pub async fn retrospect(
        &self,
        turns: &[Turn],
        total_turns: usize,
        criteria: &QualityCriteria,
    ) -> Result<RetrospectResult> {
        let trajectory = self.build_trajectory(turns, total_turns);
        let formatted = trajectory.format_for_prompt();

        let critique = self
            .critic
            .evaluate_trajectory(&formatted, criteria, None)
            .await?;

        let observation = critique
            .observation
            .clone()
            .unwrap_or_else(|| "No specific observation from trajectory review.".to_string());

        Ok(RetrospectResult {
            critique,
            observation,
            turn_count: total_turns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::turns::{ToolCallRecord, TurnUsage};
    use crate::providers::mock::MockProvider;

    fn make_turns(n: usize) -> Vec<Turn> {
        (0..n)
            .map(|i| {
                let mut turn = Turn::new(i, format!("User message {}", i));
                turn.complete(format!("Assistant response {}", i));
                turn
            })
            .collect()
    }

    #[test]
    fn test_build_trajectory_respects_window() {
        let engine = RetrospectEngine::new(
            RetrospectConfig {
                interval: 10,
                window_size: 3,
                min_turns: 3,
            },
            Arc::new(MockProvider::new()),
        );

        let turns = make_turns(10);
        let trajectory = engine.build_trajectory(&turns, 10);

        assert_eq!(trajectory.window_size, 3);
        assert_eq!(trajectory.turns.len(), 3);
        assert_eq!(trajectory.turns[0].user_message, "User message 7");
        assert_eq!(trajectory.turns[2].user_message, "User message 9");
    }

    #[test]
    fn test_build_trajectory_less_than_window() {
        let engine = RetrospectEngine::new(
            RetrospectConfig {
                interval: 10,
                window_size: 10,
                min_turns: 3,
            },
            Arc::new(MockProvider::new()),
        );

        let turns = make_turns(3);
        let trajectory = engine.build_trajectory(&turns, 3);

        assert_eq!(trajectory.window_size, 3);
        assert_eq!(trajectory.turns.len(), 3);
        assert_eq!(trajectory.total_turns, 3);
    }

    #[test]
    fn test_build_trajectory_empty_turns() {
        let engine = RetrospectEngine::new(
            RetrospectConfig {
                interval: 10,
                window_size: 5,
                min_turns: 3,
            },
            Arc::new(MockProvider::new()),
        );

        let turns: Vec<Turn> = vec![];
        let trajectory = engine.build_trajectory(&turns, 0);

        assert_eq!(trajectory.window_size, 0);
        assert!(trajectory.turns.is_empty());
        assert_eq!(trajectory.total_turns, 0);
    }

    #[test]
    fn test_build_trajectory_includes_assistant_response() {
        let engine = RetrospectEngine::new(
            RetrospectConfig {
                interval: 10,
                window_size: 1,
                min_turns: 3,
            },
            Arc::new(MockProvider::new()),
        );

        let mut turn = Turn::new(0, "Hello");
        turn.complete("Hi there!");
        let trajectory = engine.build_trajectory(&[turn], 1);

        assert_eq!(trajectory.turns.len(), 1);
        assert_eq!(trajectory.turns[0].steps.len(), 1);
        match &trajectory.turns[0].steps[0] {
            TrajectoryStep::AssistantResponse { content } => {
                assert_eq!(content, "Hi there!");
            }
            _ => panic!("Expected AssistantResponse step"),
        }
    }

    #[test]
    fn test_build_trajectory_empty_assistant_skips_step() {
        let engine = RetrospectEngine::new(
            RetrospectConfig {
                interval: 10,
                window_size: 1,
                min_turns: 3,
            },
            Arc::new(MockProvider::new()),
        );

        // A turn with no assistant response yet
        let turn = Turn::new(0, "Hello");
        let trajectory = engine.build_trajectory(&[turn], 1);

        assert_eq!(trajectory.turns.len(), 1);
        assert!(trajectory.turns[0].steps.is_empty());
    }

    #[test]
    fn test_build_trajectory_includes_tool_calls() {
        let engine = RetrospectEngine::new(
            RetrospectConfig {
                interval: 10,
                window_size: 1,
                min_turns: 3,
            },
            Arc::new(MockProvider::new()),
        );

        let mut turn = Turn::new(0, "Search for Rust");
        turn.tool_calls.push(ToolCallRecord {
            name: "search_web".to_string(),
            args: r#"{"query": "Rust"}"#.to_string(),
            result: "Rust is a systems language…".to_string(),
            success: true,
            duration_ms: 1500,
        });
        turn.complete("Here's what I found.");

        let trajectory = engine.build_trajectory(&[turn], 1);

        assert_eq!(trajectory.turns[0].steps.len(), 3); // ToolCall + ToolResult + AssistantResponse + TokenUsage
        match &trajectory.turns[0].steps[0] {
            TrajectoryStep::ToolCall { name, args, duration_ms } => {
                assert_eq!(name, "search_web");
                assert!(args.contains("Rust"));
                assert_eq!(*duration_ms, 1500);
            }
            _ => panic!("Expected ToolCall as first step"),
        }
        match &trajectory.turns[0].steps[1] {
            TrajectoryStep::ToolResult { name, content, success } => {
                assert_eq!(name, "search_web");
                assert_eq!(content, "Rust is a systems language…");
                assert!(*success);
            }
            _ => panic!("Expected ToolResult as second step"),
        }
    }

    #[test]
    fn test_build_trajectory_includes_token_usage() {
        let engine = RetrospectEngine::new(
            RetrospectConfig {
                interval: 10,
                window_size: 1,
                min_turns: 3,
            },
            Arc::new(MockProvider::new()),
        );

        let mut turn = Turn::new(0, "Hello");
        turn.token_usage = Some(TurnUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        });
        turn.complete("Hi there!");

        let trajectory = engine.build_trajectory(&[turn], 1);

        let token_steps: Vec<_> = trajectory.turns[0]
            .steps
            .iter()
            .filter_map(|s| match s {
                TrajectoryStep::TokenUsage {
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                } => Some((*prompt_tokens, *completion_tokens, *total_tokens)),
                _ => None,
            })
            .collect();
        assert_eq!(token_steps, vec![(100, 50, 150)]);
    }

    #[test]
    fn test_build_trajectory_tool_calls_ordered_before_assistant() {
        let engine = RetrospectEngine::new(
            RetrospectConfig {
                interval: 10,
                window_size: 1,
                min_turns: 3,
            },
            Arc::new(MockProvider::new()),
        );

        let mut turn = Turn::new(0, "Do something");
        turn.tool_calls.push(ToolCallRecord {
            name: "web_fetch".to_string(),
            args: "{}".to_string(),
            result: "data".to_string(),
            success: false,
            duration_ms: 500,
        });
        turn.complete("I did it.");

        let trajectory = engine.build_trajectory(&[turn], 1);

        let steps = &trajectory.turns[0].steps;
        assert!(matches!(steps[0], TrajectoryStep::ToolCall { .. }));
        assert!(matches!(steps[1], TrajectoryStep::ToolResult { .. }));
        assert!(matches!(steps[2], TrajectoryStep::AssistantResponse { .. }));
    }
}
