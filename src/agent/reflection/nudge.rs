//! Nudge engine — periodic, background trajectory reflection.
//!
//! The nudge engine runs as a background task every N turns, reviewing the
//! last M turns of conversation and writing interaction patterns to memory.
//! Unlike synchronous per-response refinement approaches,
//! it never blocks the response and never modifies output content.

use std::sync::Arc;

use crate::providers::Provider;
use crate::Result;

use super::config::NudgeConfig;
use super::critic::Critic;
use super::trajectory::{Trajectory, TrajectoryStep, TrajectoryWindow};
use super::types::{Critique, QualityCriteria};
use crate::agent::turns::Turn;

/// Result of a nudge cycle — trajectory critique + observation for memory.
#[derive(Debug, Clone)]
pub struct NudgeResult {
    /// The structured critique of the conversation window.
    pub critique: Critique,
    /// Natural-language observation extracted from the critique.
    /// This is the key output persisted to memory.
    pub observation: String,
    /// Turn count at which this nudge fired.
    pub turn_count: usize,
}

/// Periodic trajectory reflection engine.
///
/// Runs every [`NudgeConfig::interval`] turns, reviewing the last
/// [`NudgeConfig::window_size`] turns to identify interaction patterns.
#[derive(Clone)]
pub struct NudgeEngine {
    /// Nudge scheduling and window configuration.
    pub config: NudgeConfig,
    /// The LLM critic used for trajectory evaluation.
    critic: Critic,
}

impl NudgeEngine {
    /// Create a new nudge engine with the given config and provider.
    pub fn new(config: NudgeConfig, provider: Arc<dyn Provider>) -> Self {
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
    /// Each turn's `user_message` and `assistant_response` are mapped to
    /// [`TrajectoryStep`] entries. Turns beyond the configured window size
    /// are discarded.
    pub fn build_trajectory(&self, turns: &[Turn], total_turns: usize) -> Trajectory {
        let window = self.config.window_size.min(turns.len());
        let recent = &turns[turns.len().saturating_sub(window)..];

        let trajectory_turns: Vec<TrajectoryWindow> = recent
            .iter()
            .map(|turn| {
                let mut steps = Vec::new();
                if !turn.assistant_response.is_empty() {
                    steps.push(TrajectoryStep::AssistantResponse {
                        content: turn.assistant_response.clone(),
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
    /// Returns a [`NudgeResult`] with the critique and observation.
    pub async fn nudge(
        &self,
        turns: &[Turn],
        total_turns: usize,
        criteria: &QualityCriteria,
    ) -> Result<NudgeResult> {
        let trajectory = self.build_trajectory(turns, total_turns);
        let formatted = trajectory.format_for_prompt();

        let critique = self
            .critic
            .evaluate_trajectory(&formatted, criteria)
            .await?;

        let observation = critique
            .observation
            .clone()
            .unwrap_or_else(|| "No specific observation from trajectory review.".to_string());

        Ok(NudgeResult {
            critique,
            observation,
            turn_count: total_turns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let engine = NudgeEngine::new(
            NudgeConfig {
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
        let engine = NudgeEngine::new(
            NudgeConfig {
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
        let engine = NudgeEngine::new(
            NudgeConfig {
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
        let engine = NudgeEngine::new(
            NudgeConfig {
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
        let engine = NudgeEngine::new(
            NudgeConfig {
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
}
