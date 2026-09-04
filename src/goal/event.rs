//! Goal events — progress updates emitted by the
//! [`GoalRunner`](super::runner::GoalRunner).
//!
//! These events flow through the gateway's event system to the originating
//! session.

use crate::goal::condition::CheckResult;

/// Machine-readable code for why a goal was aborted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockedReasonCode {
    /// The same conditions failed with identical output N rounds in a row.
    LoopDetected,
    /// The goal exhausted its round budget.
    MaxRounds,
    /// The round-driving agent errored.
    AgentError,
    /// A human cancelled the goal.
    Cancelled,
    /// Fresh-context mode: a round's structured handoff was missing,
    /// malformed, over-limit, or violated its schema. Rejected outright —
    /// never truncated or interpreted.
    InvalidHandoff,
    /// Fatal configuration problem (e.g. no provider configured for the
    /// plan's model). Resuming requires fixing the configuration first.
    FatalConfigError,
}

/// Structured abort reason: a stable code for programmatic consumers plus a
/// human-readable message. Carried on `goal.aborted` events and persisted in
/// the goal's checkpoint file when the goal is blocked (loop/max-rounds) so
/// the cause survives and the goal can be resumed deliberately.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockedReason {
    pub code: BlockedReasonCode,
    pub message: String,
}

/// Events emitted by a [`GoalRunner`](super::runner::GoalRunner) during
/// execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event")]
pub enum GoalEvent {
    /// Goal execution has started (includes the parsed plan).
    #[serde(rename = "goal.started")]
    Started {
        id: String,
        description: String,
        conditions: Vec<String>,
        max_rounds: usize,
    },
    /// A round of checks completed.
    #[serde(rename = "goal.check")]
    Check {
        round: usize,
        results: Vec<CheckResult>,
        passed: usize,
        total: usize,
    },
    /// Starting a new round with feedback from the previous one.
    #[serde(rename = "goal.retry")]
    Retry { round: usize, feedback: String },
    /// Goal completed successfully.
    #[serde(rename = "goal.done")]
    Done {
        total_rounds: usize,
        all_passed: bool,
        summary: String,
        /// Cumulative executor token spend (cost axis) — `None` when no
        /// provider echoed usage. Additive; existing consumers ignore it.
        #[serde(default)]
        token_usage: Option<crate::agent::turns::TurnUsage>,
    },
    /// Goal aborted (max rounds, loop detected, cancelled, error).
    #[serde(rename = "goal.aborted")]
    Aborted {
        reason: String,
        round: usize,
        results: Vec<CheckResult>,
        /// Structured form of `reason` — additive; `reason` stays as-is for
        /// existing consumers.
        #[serde(default)]
        blocked_reason: Option<BlockedReason>,
        /// Cumulative executor token spend (cost axis) — `None` when no
        /// provider echoed usage. Additive; existing consumers ignore it.
        #[serde(default)]
        token_usage: Option<crate::agent::turns::TurnUsage>,
    },
}
