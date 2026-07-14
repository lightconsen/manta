//! Eval Harness — multi-trial execution engine.
//!
//! Runs N independent trials per EvalTask, scoring each with:
//! 1. GoalCondition (Code Scorer) — deterministic checks
//! 2. Critic (LLM Judge) — semantic quality dimensions
//!
//! After N trials, produces EvalSummary with pass rate, Wilson CI,
//! continuous success rate, and per-dimension averages.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::agent::reflection::critic::Critic;
use crate::agent::reflection::trajectory::{Trajectory, TrajectoryStep, TrajectoryWindow};
use crate::agent::reflection::types::Critique;
use crate::agent::turns::Turn;
use crate::agent::Agent;
use crate::channels::{
    ConversationId, IncomingMessage, InputProvenance, MentionState, MessageMetadata,
    OutgoingMessage, UserId,
};
use crate::core::models::Id;
use crate::eval::dataset::EvalTask;
use crate::goal::condition::CheckResult;
use crate::Result;

// ── Trial-level types ───────────────────────────────────────────────────

/// Summary of a single tool call (for reporting).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub name: String,
    pub args: String,
    pub result: String,
    pub success: bool,
    pub duration_ms: u64,
}

/// Result of a single trial.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrialResult {
    /// 0-based trial index.
    pub trial_index: usize,
    /// Agent's text response.
    pub response: String,
    /// Tool calls made during this trial.
    pub tool_calls: Vec<ToolCallSummary>,
    /// Token usage (if available).
    pub token_usage: Option<TurnUsage>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,

    // ── Scoring ──

    /// Per-condition check results from GoalCondition.
    pub condition_results: Vec<CheckResult>,
    /// Whether all GoalConditions passed.
    pub conditions_passed: bool,
    /// LLM Judge critique (if criteria were provided).
    pub critique: Option<Critique>,
    /// Whether the critique passed all thresholds.
    pub critique_passed: bool,

    /// Composite pass/fail (conditions_passed && critique_passed).
    pub passed: bool,
}

/// Simplified token usage for reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// ── Summary types ───────────────────────────────────────────────────────

/// Statistical summary over N trials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSummary {
    pub task_id: String,
    pub total_trials: usize,

    // ── Core metrics ──

    /// Fraction of trials that passed.
    pub pass_rate: f64,
    /// Whether at least one trial passed (capability upper bound).
    pub at_least_once_success: bool,
    /// Whether ALL trials passed (production stability metric).
    pub continuous_success: bool,

    /// 95% Wilson score confidence interval (lower, upper).
    pub confidence_interval: (f64, f64),

    /// Average per-dimension scores from LLM Judge.
    pub avg_dimension_scores: HashMap<String, f64>,
    /// Average wall-clock duration across trials.
    pub avg_duration_ms: f64,
    /// Average token usage (if available).
    pub avg_token_usage: Option<TurnUsage>,

    /// Detailed per-trial results.
    pub per_trial: Vec<TrialResult>,

    /// When this evaluation ran.
    pub completed_at: SystemTime,
}

impl EvalSummary {
    /// Compute statistical summary from N trial results.
    pub fn compute(task_id: String, results: Vec<TrialResult>) -> Self {
        let total = results.len();
        if total == 0 {
            return Self {
                task_id,
                total_trials: 0,
                pass_rate: 0.0,
                at_least_once_success: false,
                continuous_success: false,
                confidence_interval: (0.0, 0.0),
                avg_dimension_scores: HashMap::new(),
                avg_duration_ms: 0.0,
                avg_token_usage: None,
                per_trial: results,
                completed_at: SystemTime::now(),
            };
        }

        let passes: Vec<bool> = results.iter().map(|r| r.passed).collect();
        let pass_count = passes.iter().filter(|&&p| p).count();

        // Core metrics
        let pass_rate = pass_count as f64 / total as f64;
        let at_least_once_success = passes.iter().any(|&p| p);
        let continuous_success = passes.iter().all(|&p| p);

        // Wilson score interval (95% confidence)
        let ci = Self::wilson_ci(pass_count, total, 1.96);

        // Average dimension scores from LLM Judge
        let mut dim_scores: HashMap<String, Vec<f64>> = HashMap::new();
        for r in &results {
            if let Some(ref c) = r.critique {
                for (dim, score) in &c.dimension_scores {
                    dim_scores.entry(dim.clone()).or_default().push(*score);
                }
            }
        }
        let avg_dims = dim_scores
            .into_iter()
            .map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64))
            .collect();

        // Average duration
        let avg_ms = results.iter().map(|r| r.duration_ms).sum::<u64>() as f64 / total as f64;

        // Average token usage (first available)
        let avg_token = {
            let mut pt: u64 = 0;
            let mut ct: u64 = 0;
            let mut count = 0;
            for r in &results {
                if let Some(ref tu) = r.token_usage {
                    pt += tu.prompt_tokens as u64;
                    ct += tu.completion_tokens as u64;
                    count += 1;
                }
            }
            if count > 0 {
                Some(TurnUsage {
                    prompt_tokens: (pt / count) as u32,
                    completion_tokens: (ct / count) as u32,
                    total_tokens: ((pt + ct) / count) as u32,
                })
            } else {
                None
            }
        };

        Self {
            task_id,
            total_trials: total,
            pass_rate,
            at_least_once_success,
            continuous_success,
            confidence_interval: ci,
            avg_dimension_scores: avg_dims,
            avg_duration_ms: avg_ms,
            avg_token_usage: avg_token,
            per_trial: results,
            completed_at: SystemTime::now(),
        }
    }

    /// Wilson score interval for binomial proportion confidence.
    ///
    /// More accurate than normal approximation for small samples (N < 30).
    /// z = 1.96 for 95% confidence.
    fn wilson_ci(successes: usize, total: usize, z: f64) -> (f64, f64) {
        let n = total as f64;
        if n == 0.0 {
            return (0.0, 0.0);
        }
        let p = successes as f64 / n;
        let z2 = z * z;
        let denominator = 1.0 + z2 / n;
        let centre = (p + z2 / (2.0 * n)) / denominator;
        let margin = z * ((p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt()) / denominator;
        (
            (centre - margin).max(0.0),
            (centre + margin).min(1.0),
        )
    }
}

impl std::fmt::Display for EvalSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Task: {} ({} trials)", self.task_id, self.total_trials)?;
        writeln!(f, "  Pass rate:           {:.1}%", self.pass_rate * 100.0)?;
        writeln!(f, "  At-least-once:       {}", self.at_least_once_success)?;
        writeln!(f, "  Continuous success:  {}", self.continuous_success)?;
        writeln!(
            f,
            "  95% CI:              ({:.3}, {:.3})",
            self.confidence_interval.0, self.confidence_interval.1
        )?;
        if !self.avg_dimension_scores.is_empty() {
            writeln!(f, "  Avg dimensions:")?;
            for (dim, score) in &self.avg_dimension_scores {
                writeln!(f, "    {:<20} {:.2}", dim, score)?;
            }
        }
        writeln!(f, "  Avg duration:        {:.1}s", self.avg_duration_ms / 1000.0)?;
        if let Some(ref tu) = self.avg_token_usage {
            writeln!(f, "  Avg tokens:          {} ({} prompt + {} completion)", tu.total_tokens, tu.prompt_tokens, tu.completion_tokens)?;
        }
        writeln!(f, "  Per trial:")?;
        for (i, trial) in self.per_trial.iter().enumerate() {
            let status = if trial.passed { "PASS" } else { "FAIL" };
            let cond_s = if trial.conditions_passed { "✓" } else { "✗" };
            let crit_s = trial
                .critique
                .as_ref()
                .map(|c| format!("{:.2}", c.overall_score))
                .unwrap_or_else(|| "N/A".to_string());
            writeln!(f, "    #{:<3} {}  (cond={}, critique={})", i, status, cond_s, crit_s)?;
        }
        Ok(())
    }
}

// ── EvalHarness ─────────────────────────────────────────────────────────

/// Multi-trial evaluation harness.
///
/// Orchestrates:
/// 1. Agent.process_message() for each trial
/// 2. GoalCondition.check() for deterministic scoring
/// 3. Critic.evaluate_trajectory() for LLM Judge scoring
/// 4. Statistical aggregation across N trials
pub struct EvalHarness {
    agent: Arc<Agent>,
    critic: Option<Critic>,
    /// Default number of trials (can be overridden per run).
    default_trials: usize,
}

impl EvalHarness {
    /// Create a new eval harness.
    ///
    /// `critic` is optional — if not provided, only GoalCondition scoring
    /// will be used (no LLM Judge evaluation).
    pub fn new(agent: Arc<Agent>, critic: Option<Critic>) -> Self {
        Self {
            agent,
            critic,
            default_trials: 5,
        }
    }

    /// Set the default number of trials.
    pub fn with_default_trials(mut self, n: usize) -> Self {
        self.default_trials = n;
        self
    }

    /// Run a single task for N trials.
    pub async fn run(&self, task: EvalTask, n_trials: usize) -> Result<EvalSummary> {
        let n = if n_trials > 0 { n_trials } else { self.default_trials };
        info!("Running eval task '{}' ({} trials)", task.id, n);

        let mut results = Vec::with_capacity(n);
        for trial_index in 0..n {
            // Run setup commands
            Self::run_commands(&task.setup).await;

            let result = self.run_single_trial(&task, trial_index).await;

            // Run cleanup commands
            Self::run_commands(&task.cleanup).await;

            match result {
                Ok(r) => {
                    debug!("Trial {}/{} for '{}': {}", trial_index + 1, n, task.id, if r.passed { "PASS" } else { "FAIL" });
                    results.push(r);
                }
                Err(e) => {
                    warn!("Trial {}/{} for '{}' failed: {}", trial_index + 1, n, task.id, e);
                    results.push(TrialResult {
                        trial_index,
                        response: String::new(),
                        tool_calls: vec![],
                        token_usage: None,
                        duration_ms: 0,
                        condition_results: vec![],
                        conditions_passed: false,
                        critique: None,
                        critique_passed: false,
                        passed: false,
                    });
                }
            }
        }

        let summary = EvalSummary::compute(task.id.clone(), results);
        info!("Eval '{}' complete: {:.1}% pass rate", task.id, summary.pass_rate * 100.0);
        Ok(summary)
    }

    /// Execute a single trial with a fresh conversation context.
    async fn run_single_trial(&self, task: &EvalTask, trial: usize) -> Result<TrialResult> {
        let start = std::time::Instant::now();
        let conv_id = ConversationId::new(format!("eval_{}_{}", task.id, trial));

        // ── Step 1: Send message to agent with tool execution ──────────
        let msg = IncomingMessage {
            id: Id::new(),
            user_id: UserId::new(task.user_id.clone()),
            conversation_id: conv_id.clone(),
            content: task.input.clone(),
            attachments: vec![],
            metadata: MessageMetadata::new(),
            provenance: InputProvenance::ExternalUser {
                channel: "eval".into(),
                is_direct: true,
            },
            mention: MentionState::DirectMessage,
        };
        // Use process_message_with_progress (not plain process_message)
        // because process_message calls get_completion which skips the
        // tool-calling loop — no tools would be executed.
        let noop_cb: crate::agent::ProgressCallback = Arc::new(|_| {
            Box::pin(async {})
        });
        let outgoing = self.agent.process_message_with_progress(msg, noop_cb).await?;

        // ── Step 2: GoalCondition checks ──────────────────────────────
        // Write response and tool info to temp files for condition checks
        let tmp = std::env::temp_dir().join(format!("eval_{}_{}", task.id, trial));
        tokio::fs::create_dir_all(&tmp).await?;
        let response_path = tmp.join("response.txt");
        let tools_path = tmp.join("tools.json");

        let tool_summaries = Self::collect_tool_calls_from_outgoing(&outgoing);
        let all_tool_summaries = self.collect_all_tool_calls(&task.id, trial).await;

        // Use the richer summaries from thread_map if available
        let tool_calls = if all_tool_summaries.is_empty() {
            tool_summaries
        } else {
            all_tool_summaries
        };

        // Write artifacts to trial-specific directory
        tokio::fs::write(&response_path, &outgoing.content).await?;
        tokio::fs::write(
            &tools_path,
            serde_json::to_string(&tool_calls)?,
        )
        .await?;

        // Also write to eval_trace.log in trial dir (replaces old /tmp/ paths)
        tokio::fs::write(tmp.join("eval_trace.log"), format!("{:?}", tool_calls)).await?;

        // Check conditions with trial_dir substitution
        let mut condition_results = Vec::new();
        for condition in &task.conditions {
            let substituted = condition.substitute_trial_dir(&tmp);
            let result = substituted.check().await;
            condition_results.push(result);
        }

        let conditions_passed = condition_results.iter().all(|r| r.passed);

        // ── Step 3: Build trajectory ──────────────────────────────────
        let turns = self.get_thread_turns(&conv_id.0).await;

        let critique = if let Some(ref turns) = turns {
            let trajectory = Self::build_trajectory_from_turns(turns);
            let trajectory_text = trajectory.format_for_prompt();

            // ── Step 4: Critic evaluation ─────────────────────────────
            if let Some(ref critic) = self.critic {
                if let Some(ref criteria) = task.criteria {
                    critic
                        .evaluate_trajectory(&trajectory_text, criteria)
                        .await
                        .ok()
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let critique_passed = critique.as_ref().map(|c| c.passed).unwrap_or(true);
        let elapsed = start.elapsed();

        // ── Step 5: Cleanup ───────────────────────────────────────────
        // Remove trial conversation from thread_map
        {
            let mut map = self.agent.thread_map.lock().await;
            map.remove(&conv_id.0);
        }

        Ok(TrialResult {
            trial_index: trial,
            response: outgoing.content,
            tool_calls,
            token_usage: None,
            duration_ms: elapsed.as_millis() as u64,
            condition_results,
            conditions_passed,
            critique,
            critique_passed,
            passed: conditions_passed && critique_passed,
        })
    }

    // ── Helper methods ─────────────────────────────────────────────────

    /// Run setup/cleanup commands.
    async fn run_commands(commands: &[super::dataset::SetupCommand]) {
        for cmd in commands {
            let _ = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&cmd.command)
                .output()
                .await;
        }
    }

    /// Collect tool calls from OutgoingMessage.
    fn collect_tool_calls_from_outgoing(outgoing: &OutgoingMessage) -> Vec<ToolCallSummary> {
        let mut summaries = vec![];
        if let Some(ref calls) = outgoing.tool_calls {
            for tc in calls {
                summaries.push(ToolCallSummary {
                    name: tc.function.name.clone(),
                    args: tc.function.arguments.clone(),
                    result: String::new(),
                    success: true,
                    duration_ms: 0,
                });
            }
        }
        summaries
    }

    /// Collect detailed tool call records from thread_map turns.
    ///
    /// `process_message_with_progress` transfers records from the context's
    /// accumulator into `turn.tool_calls` before returning, so we read from
    /// the turn-level field here.
    async fn collect_all_tool_calls(&self, task_id: &str, trial: usize) -> Vec<ToolCallSummary> {
        let conv_id = format!("eval_{}_{}", task_id, trial);
        let mut summaries = vec![];

        if let Some(turns) = self.get_thread_turns(&conv_id).await {
            for turn in &turns {
                for tc in &turn.tool_calls {
                    summaries.push(ToolCallSummary {
                        name: tc.name.clone(),
                        args: tc.args.clone(),
                        result: tc.result.clone(),
                        success: tc.success,
                        duration_ms: tc.duration_ms,
                    });
                }
            }
        }
        summaries
    }

    /// Get turns from thread_map by conversation ID.
    async fn get_thread_turns(&self, conv_id: &str) -> Option<Vec<Turn>> {
        let map = self.agent.thread_map.lock().await;
        map.get(conv_id).map(|t| t.turns.clone())
    }

    /// Build a Trajectory from a slice of Turns.
    fn build_trajectory_from_turns(turns: &[crate::agent::turns::Turn]) -> Trajectory {
        let windows: Vec<TrajectoryWindow> = turns
            .iter()
            .map(|turn| {
                let mut steps = vec![];
                for tc in &turn.tool_calls {
                    steps.push(TrajectoryStep::ToolCall {
                        name: tc.name.clone(),
                        args: tc.args.clone(),
                        duration_ms: tc.duration_ms,
                    });
                    steps.push(TrajectoryStep::ToolResult {
                        name: tc.name.clone(),
                        content: tc.result.clone(),
                        success: tc.success,
                    });
                }
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
            total_turns: turns.len(),
            window_size: turns.len(),
            turns: windows,
        }
    }

    /// Trigger RCA when a badcase is detected.
    ///
    /// Called automatically by the harness after a failed trial if
    /// an RcaPipeline is configured.
    pub async fn on_badcase_detected(
        &self,
        result: TrialResult,
        task: &EvalTask,
    ) {
        if !result.passed {
            debug!(
                "Badcase detected: task={}, trial={}",
                task.id, result.trial_index
            );
            // RCA pipeline integration point — called from
            // downstream code that constructs an RcaPipeline.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wilson_ci_all_pass() {
        let ci = EvalSummary::wilson_ci(5, 5, 1.96);
        assert!(ci.0 >= 0.4, "Lower bound should be reasonable: {:?}", ci);
        assert!(ci.1 <= 1.0);
    }

    #[test]
    fn test_wilson_ci_all_fail() {
        let ci = EvalSummary::wilson_ci(0, 5, 1.96);
        assert_eq!(ci.0, 0.0);
    }

    #[test]
    fn test_wilson_ci_half() {
        let ci = EvalSummary::wilson_ci(3, 6, 1.96);
        assert!(ci.0 < 0.5);
        assert!(ci.1 > 0.5);
    }

    #[test]
    fn test_summary_all_pass() {
        let results = vec![
            TrialResult {
                trial_index: 0,
                response: "ok".into(),
                tool_calls: vec![],
                token_usage: None,
                duration_ms: 100,
                condition_results: vec![],
                conditions_passed: true,
                critique: None,
                critique_passed: true,
                passed: true,
            },
            TrialResult {
                trial_index: 1,
                response: "ok".into(),
                tool_calls: vec![],
                token_usage: None,
                duration_ms: 200,
                condition_results: vec![],
                conditions_passed: true,
                critique: None,
                critique_passed: true,
                passed: true,
            },
        ];
        let s = EvalSummary::compute("test".into(), results);
        assert_eq!(s.total_trials, 2);
        assert_eq!(s.pass_rate, 1.0);
        assert!(s.at_least_once_success);
        assert!(s.continuous_success);
        assert_eq!(s.avg_duration_ms, 150.0);
    }

    #[test]
    fn test_summary_partial_pass() {
        let results = vec![
            TrialResult {
                trial_index: 0,
                passed: true,
                ..make_dummy()
            },
            TrialResult {
                trial_index: 1,
                passed: false,
                ..make_dummy()
            },
        ];
        let s = EvalSummary::compute("test".into(), results);
        assert_eq!(s.pass_rate, 0.5);
        assert!(s.at_least_once_success);
        assert!(!s.continuous_success);
    }

    fn make_dummy() -> TrialResult {
        TrialResult {
            trial_index: 0,
            response: String::new(),
            tool_calls: vec![],
            token_usage: None,
            duration_ms: 0,
            condition_results: vec![],
            conditions_passed: false,
            critique: None,
            critique_passed: false,
            passed: false,
        }
    }
}
