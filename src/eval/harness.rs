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
use crate::eval::dataset::{EvalTask, SkillEvalDesign};
use crate::eval::rca::{rca_input_from_trial, BadcaseEntry, RcaPipeline};
use crate::eval::skill_scorer::{SkillCheckResult, SkillScorer};
use crate::goal::condition::{CheckResult, GoalCondition};
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

/// Result of a single turn within a multi-turn trial (§03).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    /// 0-based turn index within the trial.
    pub turn_index: usize,
    /// The user message that initiated this turn.
    pub user_message: String,
    /// Agent's text response for this turn.
    pub response: String,
    /// Tool calls made during this turn.
    pub tool_calls: Vec<ToolCallSummary>,
    /// Per-turn condition check results.
    pub condition_results: Vec<CheckResult>,
    /// Whether all per-turn conditions passed.
    pub conditions_passed: bool,
    /// Wall-clock duration for this turn in milliseconds.
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
    /// Skill evaluation results (optional — only when skill designs exist).
    pub skill_results: Option<SkillCheckResult>,
    /// Whether skill checks passed (true when no skill designs).
    pub skill_passed: bool,

    /// Composite pass/fail (conditions_passed && critique_passed && skill_passed && session_conditions_passed).
    pub passed: bool,

    // ── Session-level (§03) ──
    /// Per-turn results for multi-turn tasks.
    #[serde(default)]
    pub turn_results: Vec<TurnResult>,
    /// Session-level condition results.
    #[serde(default)]
    pub session_condition_results: Vec<CheckResult>,
    /// Whether all session-level conditions passed.
    pub session_conditions_passed: bool,
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

    // ── Skill sub-metrics (§04) ──
    /// Overall skill pass rate across all trials with skill designs.
    #[serde(default)]
    pub skill_pass_rate: f64,
    /// Trigger dimension pass rate.
    #[serde(default)]
    pub skill_trigger_pass_rate: f64,
    /// Execution dimension pass rate.
    #[serde(default)]
    pub skill_execution_pass_rate: f64,
    /// Quality dimension pass rate.
    #[serde(default)]
    pub skill_quality_pass_rate: f64,
    /// Resilience dimension pass rate.
    #[serde(default)]
    pub skill_resilience_pass_rate: f64,
    /// The 6 named sub-metrics from §04 as key-value pairs.
    #[serde(default)]
    pub skill_sub_metrics: HashMap<String, f64>,

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
                skill_pass_rate: 1.0,
                skill_trigger_pass_rate: 1.0,
                skill_execution_pass_rate: 1.0,
                skill_quality_pass_rate: 1.0,
                skill_resilience_pass_rate: 1.0,
                skill_sub_metrics: HashMap::new(),
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

        // ── Skill sub-metrics (§04) ──────────────────────────────────
        let trials_with_skills: Vec<&TrialResult> = results
            .iter()
            .filter(|r| r.skill_results.is_some())
            .collect();
        let n_skill = trials_with_skills.len();

        let skill_pass_rate = if n_skill > 0 {
            trials_with_skills.iter().filter(|r| r.skill_passed).count() as f64 / n_skill as f64
        } else {
            1.0
        };

        let skill_trig = skill_dim_pass_rate(&trials_with_skills, |sr| {
            sr.trigger_results.iter().all(|t| t.passed)
        });
        let skill_exec = skill_dim_pass_rate(&trials_with_skills, |sr| {
            sr.execution_results.iter().all(|e| e.passed)
        });
        let skill_qual = skill_dim_pass_rate(&trials_with_skills, |sr| {
            sr.quality_results.iter().all(|q| q.passed)
        });
        let skill_res = skill_dim_pass_rate(&trials_with_skills, |sr| {
            sr.resilience_results.iter().all(|r| r.passed)
        });

        // 6 named sub-metrics from §04
        let mut sub_metrics = HashMap::new();
        sub_metrics.insert("trigger_precision_recall".into(), skill_trig);
        sub_metrics.insert(
            "parameter_accuracy".into(),
            skill_dim_pass_rate(&trials_with_skills, |sr| {
                sr.execution_results.iter().all(|e| {
                    // A parameter check was involved — pass if all required params satisfied
                    e.passed
                })
            }),
        );
        sub_metrics.insert(
            "required_steps_pass_rate".into(),
            skill_dim_pass_rate(&trials_with_skills, |sr| {
                sr.execution_results.iter().all(|e| e.passed)
            }),
        );
        let violation_rate = skill_dim_pass_rate(&trials_with_skills, |sr| {
            sr.execution_results.iter().all(|e| e.passed)
        });
        sub_metrics.insert("forbidden_actions_violation_rate".into(), violation_rate);
        sub_metrics.insert("exception_tolerance_pass_rate".into(), skill_res);
        sub_metrics.insert("output_usability_rate".into(), skill_qual);

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
            skill_pass_rate,
            skill_trigger_pass_rate: skill_trig,
            skill_execution_pass_rate: skill_exec,
            skill_quality_pass_rate: skill_qual,
            skill_resilience_pass_rate: skill_res,
            skill_sub_metrics: sub_metrics,
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
        ((centre - margin).max(0.0), (centre + margin).min(1.0))
    }
}

/// Helper: compute fraction of skill trials where a dimension predicate passes.
fn skill_dim_pass_rate(trials: &[&TrialResult], pred: impl Fn(&SkillCheckResult) -> bool) -> f64 {
    if trials.is_empty() {
        return 1.0;
    }
    let passes = trials
        .iter()
        .filter(|t| t.skill_results.as_ref().is_none_or(&pred))
        .count();
    passes as f64 / trials.len() as f64
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
            writeln!(
                f,
                "  Avg tokens:          {} ({} prompt + {} completion)",
                tu.total_tokens, tu.prompt_tokens, tu.completion_tokens
            )?;
        }
        writeln!(f, "  Per trial:")?;
        for (i, trial) in self.per_trial.iter().enumerate() {
            let status = if trial.passed { "PASS" } else { "FAIL" };
            let cond_s = if trial.conditions_passed {
                "✓"
            } else {
                "✗"
            };
            let crit_s = trial
                .critique
                .as_ref()
                .map(|c| format!("{:.2}", c.overall_score))
                .unwrap_or_else(|| "N/A".to_string());
            let skill_s = if trial.skill_passed { "✓" } else { "✗" };
            let session_s = if trial.session_conditions_passed {
                "✓"
            } else {
                "✗"
            };
            let turn_info = if trial.turn_results.len() > 1 {
                format!(" {} turns", trial.turn_results.len())
            } else {
                String::new()
            };
            writeln!(
                f,
                "    #{:<3} {}  (cond={}, critique={}, skill={}, session={}){}",
                i, status, cond_s, crit_s, skill_s, session_s, turn_info
            )?;
        }

        // ── Skill breakdown (§04) ──
        if self.skill_sub_metrics.is_empty()
            && self.per_trial.iter().any(|t| t.skill_results.is_some())
        {
            // Only skill pass rate available
            writeln!(f, "  Skill metrics:")?;
            writeln!(f, "    Skill pass rate:          {:.1}%", self.skill_pass_rate * 100.0)?;
        } else if !self.skill_sub_metrics.is_empty() {
            writeln!(f, "  Skill metrics:")?;
            writeln!(f, "    Skill pass rate:          {:.1}%", self.skill_pass_rate * 100.0)?;
            writeln!(
                f,
                "    Trigger:                  {:.1}%",
                self.skill_trigger_pass_rate * 100.0
            )?;
            writeln!(
                f,
                "    Execution:                {:.1}%",
                self.skill_execution_pass_rate * 100.0
            )?;
            writeln!(
                f,
                "    Quality:                  {:.1}%",
                self.skill_quality_pass_rate * 100.0
            )?;
            writeln!(
                f,
                "    Resilience:               {:.1}%",
                self.skill_resilience_pass_rate * 100.0
            )?;
            let mut metrics: Vec<(&String, &f64)> = self.skill_sub_metrics.iter().collect();
            metrics.sort_by_key(|(k, _)| *k);
            for (key, val) in &metrics {
                writeln!(f, "    {:<30} {:.1}%", key, *val * 100.0)?;
            }
        }

        Ok(())
    }
}

// ── EvalHarness ─────────────────────────────────────────────────────────

/// Controls when EvalHarness can stop trials early (§10).
///
/// All channels are disabled by default (0 / false), preserving
/// existing behavior for all callers that don't explicitly configure it.
#[derive(Debug, Clone)]
pub struct EarlyStopConfig {
    /// Minimum trials before considering early stop (default 3).
    pub min_trials: usize,
    /// Stop after N consecutive passes (0 = disabled, default).
    pub consecutive_passes: usize,
    /// Stop after N consecutive failures (0 = disabled, default).
    pub consecutive_failures: usize,
    /// Stop immediately on first failure.
    pub continuous_success_required: bool,
}

impl Default for EarlyStopConfig {
    fn default() -> Self {
        Self {
            min_trials: 3,
            consecutive_passes: 0,
            consecutive_failures: 0,
            continuous_success_required: false,
        }
    }
}

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
    /// Skill evaluation designs to check per trial (§02).
    skill_designs: Vec<SkillEvalDesign>,
    /// Optional RCA pipeline for automatic badcase analysis (§07).
    rca_pipeline: Option<Arc<RcaPipeline>>,
    /// Early stopping configuration (§10).
    early_stop: EarlyStopConfig,
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
            skill_designs: Vec::new(),
            rca_pipeline: None,
            early_stop: EarlyStopConfig::default(),
        }
    }

    /// Set the default number of trials.
    pub fn with_default_trials(mut self, n: usize) -> Self {
        self.default_trials = n;
        self
    }

    /// Set skill evaluation designs to check per trial.
    pub fn with_skill_designs(mut self, designs: Vec<SkillEvalDesign>) -> Self {
        self.skill_designs = designs;
        self
    }

    /// Set an optional RCA pipeline for automatic badcase analysis.
    pub fn with_rca_pipeline(mut self, rca: Option<Arc<RcaPipeline>>) -> Self {
        self.rca_pipeline = rca;
        self
    }

    /// Set early stopping configuration (§10).
    pub fn with_early_stop(mut self, config: EarlyStopConfig) -> Self {
        self.early_stop = config;
        self
    }

    /// Run a single task for N trials.
    pub async fn run(&self, task: EvalTask, n_trials: usize) -> Result<EvalSummary> {
        let n = if n_trials > 0 {
            n_trials
        } else {
            self.default_trials
        };
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
                    debug!(
                        "Trial {}/{} for '{}': {}",
                        trial_index + 1,
                        n,
                        task.id,
                        if r.passed { "PASS" } else { "FAIL" }
                    );
                    self.on_badcase_detected(&r, &task).await;
                    results.push(r);
                }
                Err(e) => {
                    warn!("Trial {}/{} for '{}' failed: {}", trial_index + 1, n, task.id, e);
                    let failed = TrialResult {
                        trial_index,
                        response: String::new(),
                        tool_calls: vec![],
                        token_usage: None,
                        duration_ms: 0,
                        condition_results: vec![],
                        conditions_passed: false,
                        critique: None,
                        critique_passed: false,
                        skill_results: None,
                        skill_passed: true,
                        turn_results: vec![],
                        session_condition_results: vec![],
                        session_conditions_passed: true,
                        passed: false,
                    };
                    self.on_badcase_detected(&failed, &task).await;
                    results.push(failed);
                }
            }

            // Check if we can stop early (§10)
            if self.should_stop_early(&results, n) {
                break;
            }
        }

        let summary = EvalSummary::compute(task.id.clone(), results);
        info!("Eval '{}' complete: {:.1}% pass rate", task.id, summary.pass_rate * 100.0);
        Ok(summary)
    }

    /// Check whether we can stop trials early based on accumulated results (§10).
    fn should_stop_early(&self, results: &[TrialResult], planned: usize) -> bool {
        let n = results.len();
        if n < self.early_stop.min_trials {
            return false;
        }
        if n >= planned {
            return true; // all planned trials completed
        }

        let cfg = &self.early_stop;

        // continuous_success_required: stop on first failure
        if cfg.continuous_success_required && results.iter().any(|r| !r.passed) {
            info!(
                "Early stopping after {}/{} trials: continuous_success_required violated",
                n, planned
            );
            return true;
        }

        // consecutive_passes: last N trials all passed
        if cfg.consecutive_passes > 0 && n >= cfg.consecutive_passes {
            let last = &results[n - cfg.consecutive_passes..];
            if last.iter().all(|r| r.passed) {
                info!(
                    "Early stopping after {}/{} trials: {} consecutive passes",
                    n, planned, cfg.consecutive_passes
                );
                return true;
            }
        }

        // consecutive_failures: last N trials all failed
        if cfg.consecutive_failures > 0 && n >= cfg.consecutive_failures {
            let last = &results[n - cfg.consecutive_failures..];
            if last.iter().all(|r| !r.passed) {
                info!(
                    "Early stopping after {}/{} trials: {} consecutive failures",
                    n, planned, cfg.consecutive_failures
                );
                return true;
            }
        }

        false
    }

    /// Execute a single trial with a fresh conversation context.
    ///
    /// For multi-turn tasks (EvalTask.turns non-empty), sends each turn
    /// sequentially within the same conversation and collects per-turn
    /// results. For single-turn tasks (backward compat), behavior is
    /// identical to the previous implementation.
    async fn run_single_trial(&self, task: &EvalTask, trial: usize) -> Result<TrialResult> {
        let start = std::time::Instant::now();
        let conv_id = ConversationId::new(format!("eval_{}_{}", task.id, trial));
        let tmp = std::env::temp_dir().join(format!("eval_{}_{}", task.id, trial));
        tokio::fs::create_dir_all(&tmp).await?;

        let noop_cb: crate::agent::ProgressCallback = Arc::new(|_| Box::pin(async {}));

        // ── Step 1: Determine turns to execute ────────────────────────
        let is_multi_turn = !task.turns.is_empty();
        let turn_specs: Vec<(&str, Option<&Vec<GoalCondition>>)> = if is_multi_turn {
            task.turns
                .iter()
                .map(|t| (t.user_message.as_str(), Some(&t.conditions)))
                .collect()
        } else {
            vec![(&task.input, None)]
        };

        // ── Step 2: Execute turns sequentially ─────────────────────────
        let mut turn_results: Vec<TurnResult> = Vec::new();
        let mut all_tool_calls: Vec<ToolCallSummary> = Vec::new();
        let mut final_response = String::new();

        for (i, (user_message, per_turn_conds)) in turn_specs.iter().enumerate() {
            let turn_start = std::time::Instant::now();

            // 2a. Send message (same conversation_id across all turns)
            let msg = IncomingMessage {
                id: Id::new(),
                user_id: UserId::new(task.user_id.clone()),
                conversation_id: conv_id.clone(),
                content: user_message.to_string(),
                attachments: vec![],
                metadata: MessageMetadata::new(),
                provenance: InputProvenance::ExternalUser {
                    channel: "eval".into(),
                    is_direct: true,
                },
                mention: MentionState::DirectMessage,
            };
            let outgoing = self
                .agent
                .process_message_with_progress(msg, noop_cb.clone())
                .await?;

            // 2b. Collect tool calls for this turn from the outgoing message
            let turn_tool_calls = Self::collect_tool_calls_from_outgoing(&outgoing);
            final_response = outgoing.content.clone();

            // Accumulate all tool calls
            all_tool_calls.extend(turn_tool_calls.clone());

            // 2c. Write per-turn artifacts
            let turn_dir = tmp.join(format!("turn_{}", i));
            tokio::fs::create_dir_all(&turn_dir).await?;
            tokio::fs::write(turn_dir.join("response.txt"), &outgoing.content).await?;
            tokio::fs::write(turn_dir.join("tools.json"), serde_json::to_string(&turn_tool_calls)?)
                .await?;

            // 2d. Check per-turn conditions
            let mut turn_condition_results = Vec::new();
            if let Some(conds) = per_turn_conds {
                for condition in *conds {
                    let substituted = condition.substitute_trial_dir(&tmp);
                    let result = substituted.check().await;
                    turn_condition_results.push(result);
                }
            }
            let turn_conditions_passed = turn_condition_results.iter().all(|r| r.passed);

            // 2e. Accumulate turn-level result
            let turn_ms = turn_start.elapsed().as_millis() as u64;
            turn_results.push(TurnResult {
                turn_index: i,
                user_message: user_message.to_string(),
                response: outgoing.content,
                tool_calls: turn_tool_calls,
                condition_results: turn_condition_results,
                conditions_passed: turn_conditions_passed,
                duration_ms: turn_ms,
            });
        }

        // ── Step 3: Write session-level artifacts ──────────────────────
        tokio::fs::write(tmp.join("response.txt"), &final_response).await?;
        tokio::fs::write(tmp.join("tools.json"), serde_json::to_string(&all_tool_calls)?).await?;
        tokio::fs::write(tmp.join("eval_trace.log"), format!("{:?}", all_tool_calls)).await?;

        // ── Step 4: GoalCondition checks (backward compat) ─────────────
        let mut condition_results = Vec::new();
        for condition in &task.conditions {
            let substituted = condition.substitute_trial_dir(&tmp);
            let result = substituted.check().await;
            condition_results.push(result);
        }
        let conditions_passed = condition_results.iter().all(|r| r.passed);

        // ── Step 5: Session-level condition checks (§03) ───────────────
        let mut session_condition_results = Vec::new();
        for condition in &task.session_conditions {
            let substituted = condition.substitute_trial_dir(&tmp);
            let result = substituted.check().await;
            session_condition_results.push(result);
        }
        let session_conditions_passed = session_condition_results.iter().all(|r| r.passed);

        // ── Step 6: Fetch thread turns for real tool call data ──────────
        let turns = self.get_thread_turns(&conv_id.0).await;
        let real_tool_calls = if let Some(ref turns) = turns {
            Self::collect_tool_calls_from_turns(turns)
        } else {
            all_tool_calls.clone() // fallback to outgoing stubs
        };

        // ── Step 7: Skill evaluation (§02 / §04) ───────────────────────
        let (skill_results, skill_passed) = self
            .evaluate_skills(&real_tool_calls, &final_response)
            .await;

        // ── Step 8: Build trajectory ──────────────────────────────────
        let critique = if let Some(ref turns) = turns {
            let trajectory = Self::build_trajectory_from_turns(turns);
            let trajectory_text = trajectory.format_for_prompt();

            // ── Step 8: Critic evaluation ────────────────────────────
            if let Some(ref critic) = self.critic {
                if let Some(ref criteria) = task.criteria {
                    critic
                        .evaluate_trajectory(&trajectory_text, criteria, task.agent_type.as_ref())
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

        // ── Step 9: Cleanup ──────────────────────────────────────────
        {
            let mut map = self.agent.thread_map.lock().await;
            map.remove(&conv_id.0);
        }

        Ok(TrialResult {
            trial_index: trial,
            response: final_response,
            tool_calls: all_tool_calls,
            token_usage: None,
            duration_ms: elapsed.as_millis() as u64,
            condition_results,
            conditions_passed,
            critique,
            critique_passed,
            skill_results,
            skill_passed,
            passed: conditions_passed
                && critique_passed
                && skill_passed
                && session_conditions_passed,
            turn_results,
            session_condition_results,
            session_conditions_passed,
        })
    }

    // ── Skill evaluation (§02) ────────────────────────────────────────

    /// Evaluate all skill designs against trial tool calls and response.
    ///
    /// Returns `(Option<SkillCheckResult>, passed)` — `passed` is `true`
    /// when there are no skill designs to check.
    async fn evaluate_skills(
        &self,
        tool_calls: &[ToolCallSummary],
        response: &str,
    ) -> (Option<SkillCheckResult>, bool) {
        if self.skill_designs.is_empty() {
            return (None, true);
        }

        // For now, evaluate against the first design only (multi-design
        // aggregation can be added later).
        let design = &self.skill_designs[0];
        let result = SkillScorer::evaluate(design, tool_calls, response).await;
        (Some(result.clone()), result.passed)
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

    /// Collect tool calls from Turn records, which carry real
    /// result/success/duration data from agent execution (§04).
    fn collect_tool_calls_from_turns(turns: &[Turn]) -> Vec<ToolCallSummary> {
        turns
            .iter()
            .flat_map(|turn| {
                turn.tool_calls.iter().map(|tcr| ToolCallSummary {
                    name: tcr.name.clone(),
                    args: tcr.args.clone(),
                    result: tcr.result.clone(),
                    success: tcr.success,
                    duration_ms: tcr.duration_ms,
                })
            })
            .collect()
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
    /// Automatically runs the 5-step RCA pipeline if configured,
    /// then persists the result to MemoryStore.
    pub async fn on_badcase_detected(&self, result: &TrialResult, task: &EvalTask) {
        if !result.passed {
            debug!("Badcase detected: task={}, trial={}", task.id, result.trial_index);

            if let Some(ref rca) = self.rca_pipeline {
                let input = rca_input_from_trial(&task.id, result, &task.input);
                match rca.analyze(input).await {
                    Ok(rca_result) => {
                        info!(
                            "RCA complete: task={}, phenomenon={:?}, module={:?}",
                            task.id, rca_result.problem_category, rca_result.responsibility_module
                        );
                        if let Err(e) = rca.persist(rca_result).await {
                            warn!("RCA persist failed: {}", e);
                        }
                    }
                    Err(e) => warn!("RCA failed for task='{}': {}", task.id, e),
                }
            }
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
                skill_results: None,
                skill_passed: true,
                turn_results: vec![],
                session_condition_results: vec![],
                session_conditions_passed: true,
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
                skill_results: None,
                skill_passed: true,
                turn_results: vec![],
                session_condition_results: vec![],
                session_conditions_passed: true,
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
            skill_results: None,
            skill_passed: true,
            turn_results: vec![],
            session_condition_results: vec![],
            session_conditions_passed: true,
            passed: false,
        }
    }
}
