//! Goal events — progress updates emitted by the [`GoalRunner`](super::runner::GoalRunner).
//!
//! These events flow through the gateway's event system to the originating session.

use crate::goal::condition::CheckResult;

/// Events emitted by a [`GoalRunner`](super::runner::GoalRunner) during execution.
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
    Retry {
        round: usize,
        feedback: String,
    },
    /// Goal completed successfully.
    #[serde(rename = "goal.done")]
    Done {
        total_rounds: usize,
        all_passed: bool,
        summary: String,
    },
    /// Goal aborted (max rounds, loop detected, cancelled, error).
    #[serde(rename = "goal.aborted")]
    Aborted {
        reason: String,
        round: usize,
        results: Vec<CheckResult>,
    },
}
