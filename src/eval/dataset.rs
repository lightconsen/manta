//! Eval dataset types — EvalTask, EvalSuite, YAML loading.
//!
//! Defines the data structures for evaluation tasks and suites,
//! organized by the article's four dataset sources:
//!   ExpertDesign, Extended, Online, BadcaseRecycle.

use crate::agent::reflection::types::QualityCriteria;
use crate::eval::agent_type::AgentType;
use crate::goal::condition::GoalCondition;

/// Source category for an evaluation task.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum EvalTaskSource {
    #[default]
    /// Expert-designed golden cases (50–200 core cases).
    #[serde(rename = "expert")]
    ExpertDesign,
    /// Extended cases generated from templates + LLM.
    #[serde(rename = "extended")]
    Extended,
    /// Real online conversation data.
    #[serde(rename = "online")]
    Online,
    /// Badcase recycled from production / human review.
    #[serde(rename = "badcase")]
    BadcaseRecycle,
}

/// Suite category — determines purpose and pass threshold.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SuiteCategory {
    #[default]
    /// Core capability verification. Target pass rate: 30–50%.
    #[serde(rename = "capability")]
    Capability,
    /// Regression protection. Target pass rate: 100%.
    #[serde(rename = "regression")]
    Regression,
    /// Safety / edge-case stress testing.
    #[serde(rename = "adversarial")]
    Adversarial,
    /// Multi-turn hard cases — measure upper bound.
    #[serde(rename = "multi_turn")]
    MultiTurnHard,
}

/// A single turn in a multi-turn evaluation task (§03).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnInput {
    /// User message for this turn.
    pub user_message: String,
    /// Per-turn GoalCondition checks (e.g. tool was called in this turn).
    #[serde(default)]
    pub conditions: Vec<GoalCondition>,
}

/// A single evaluation task.
///
/// For single-turn tasks (the default), `input` is used directly.
/// For multi-turn tasks, `turns` overrides `input` and each turn is sent
/// sequentially within the same conversation for session-level evaluation (§03).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EvalTask {
    /// Unique task identifier.
    pub id: String,
    /// Human-readable description of the scenario.
    #[serde(default)]
    pub description: String,
    /// User message sent to the agent (single-turn, backward compat).
    pub input: String,
    /// Optional user ID (defaults to "eval_user").
    #[serde(default = "default_user_id")]
    pub user_id: String,
    /// Code Scorer conditions — deterministic checks via GoalCondition.
    #[serde(default)]
    pub conditions: Vec<GoalCondition>,
    /// LLM Judge criteria for semantic scoring.
    #[serde(default)]
    pub criteria: Option<QualityCriteria>,
    /// Human-readable expected behavior (for reporting / badcase analysis).
    #[serde(default)]
    pub expected_behavior: String,
    /// Source of this task.
    #[serde(default)]
    pub source: EvalTaskSource,
    /// Failure reason for badcase-recycled tasks.
    #[serde(default)]
    pub failure_reason: Option<String>,
    /// Optional setup commands to run before the trial.
    #[serde(default)]
    pub setup: Vec<SetupCommand>,
    /// Optional cleanup commands to run after the trial.
    #[serde(default)]
    pub cleanup: Vec<SetupCommand>,
    /// Agent type for type-specific scoring emphasis (§02).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
    /// Multi-turn input sequence (§03). Overrides `input` when non-empty.
    #[serde(default)]
    pub turns: Vec<TurnInput>,
    /// Session-level conditions checked after all turns complete (§03).
    #[serde(default)]
    pub session_conditions: Vec<GoalCondition>,
}

fn default_user_id() -> String {
    "eval_user".to_string()
}

impl Default for EvalTask {
    fn default() -> Self {
        Self {
            id: String::new(),
            description: String::new(),
            input: String::new(),
            user_id: default_user_id(),
            conditions: Vec::new(),
            criteria: None,
            expected_behavior: String::new(),
            source: EvalTaskSource::ExpertDesign,
            failure_reason: None,
            setup: Vec::new(),
            cleanup: Vec::new(),
            agent_type: None,
            turns: Vec::new(),
            session_conditions: Vec::new(),
        }
    }
}

/// A setup or cleanup command to run as part of a task.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetupCommand {
    /// Shell command to execute.
    pub command: String,
}

/// A collection of eval tasks forming a test suite.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EvalSuite {
    /// Suite identifier.
    pub id: String,
    /// Human-readable name.
    #[serde(default)]
    pub name: String,
    /// Category for pass threshold / purpose.
    #[serde(default)]
    pub category: SuiteCategory,
    /// Tasks in this suite.
    #[serde(default)]
    pub tasks: Vec<EvalTask>,
    /// Minimum pass rate required for this suite.
    #[serde(default = "default_min_pass_rate")]
    pub min_pass_rate: f64,
    /// Number of trials per task (default: 5).
    #[serde(default = "default_trials")]
    pub trials: usize,
    /// Whether continuous success is required (all trials pass).
    #[serde(default)]
    pub continuous_success_required: bool,
    /// Fraction of tasks to run (0.0–1.0). 1.0 = all tasks (§10).
    #[serde(default = "default_sampling_rate")]
    pub sampling_rate: f64,
    /// Optional tags for filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Default agent type for tasks in this suite (§02).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
    /// Skill evaluation designs collected from referenced task files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_designs: Vec<SkillEvalDesign>,
}

fn default_min_pass_rate() -> f64 {
    0.8
}

fn default_trials() -> usize {
    5
}

fn default_sampling_rate() -> f64 {
    1.0
}

// ── Skill evaluation types (from §06-6 / §04) ──────────────────────────

/// Four-dimensional skill evaluation design (§04).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillEvalDesign {
    /// Trigger accuracy: should / should not trigger.
    #[serde(default)]
    pub trigger: Vec<TriggerCase>,
    /// Core logic: parameter correctness, required paths.
    #[serde(default)]
    pub execution: Vec<ExecutionCase>,
    /// Output quality: completeness, structure.
    #[serde(default)]
    pub quality: Vec<QualityCase>,
    /// Resilience: timeout, error, empty result handling.
    #[serde(default)]
    pub resilience: Vec<ResilienceCase>,
}

/// Trigger accuracy check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TriggerCase {
    /// Scenario that should trigger the skill.
    #[serde(default)]
    pub should_trigger: Option<ShouldTriggerCase>,
    /// Scenario that should NOT trigger the skill.
    #[serde(default)]
    pub should_not_trigger: Option<NoTriggerCase>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShouldTriggerCase {
    pub input: String,
    pub expect_tool: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NoTriggerCase {
    pub input: String,
    #[serde(default)]
    pub expect_no_tool: String,
}

/// Execution correctness check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExecutionCase {
    pub scenario: String,
    #[serde(default)]
    pub required_tools: Vec<String>,
    #[serde(default)]
    pub forbidden_tools: Vec<String>,
    #[serde(default)]
    pub required_params: Vec<ParamMatcher>,
    #[serde(default)]
    pub evidence_consistency: bool,
}

/// Parameter matching pattern.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParamMatcher {
    pub key: String,
    #[serde(default)]
    pub contains: Option<String>,
    #[serde(default)]
    pub equals: Option<String>,
}

/// Output quality check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QualityCase {
    pub name: String,
    #[serde(default)]
    pub must_contain: Vec<String>,
    #[serde(default)]
    pub must_not_contain: Vec<String>,
    #[serde(default)]
    pub min_length: Option<usize>,
}

/// Resilience / failure mode check.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResilienceCase {
    pub inject: FailureMode,
    pub expect: DegradeExpectation,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum FailureMode {
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "error")]
    Error(String),
    #[serde(rename = "empty_result")]
    EmptyResult,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DegradeExpectation {
    #[serde(rename = "graceful")]
    GracefulMessage(String),
    #[serde(rename = "retry")]
    Retry,
    #[serde(rename = "fallback")]
    Fallback(String),
}
