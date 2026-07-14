//! Skill-level evaluation suite (§04 / §06-6).
//!
//! Four-dimensional evaluation for individual tools:
//!   Trigger → Execution → Quality → Resilience

use serde::{Deserialize, Serialize};

/// Four-dimensional skill evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEvalSuite {
    pub skill_name: String,
    #[serde(default)]
    pub trigger: Vec<TriggerCheck>,
    #[serde(default)]
    pub execution: Vec<ExecutionCheck>,
    #[serde(default)]
    pub quality: Vec<QualityCheck>,
    #[serde(default)]
    pub resilience: Vec<ResilienceCheck>,
}

/// Trigger accuracy check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerCheck {
    pub input: String,
    pub expect_trigger: Option<bool>, // true = should trigger, false = should not
    pub expected_tool: Option<String>,
}

/// Execution correctness check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionCheck {
    pub scenario: String,
    #[serde(default)]
    pub required_params: Vec<ParamCheck>,
    #[serde(default)]
    pub expected_result_contains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamCheck {
    pub key: String,
    pub expected_value: Option<String>,
    pub expected_present: bool,
}

/// Output quality check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCheck {
    pub name: String,
    #[serde(default)]
    pub must_contain: Vec<String>,
    #[serde(default)]
    pub must_not_contain: Vec<String>,
}

/// Resilience check (error handling).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResilienceCheck {
    /// Failure mode to inject.
    pub failure_mode: FailureMode,
    /// Expected graceful behavior.
    pub expect_graceful: bool,
    #[serde(default)]
    pub expected_message_contains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailureMode {
    Timeout,
    NetworkError,
    InvalidInput,
    EmptyResult,
    RateLimited,
}
