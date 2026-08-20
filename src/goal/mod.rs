//! Goal-based execution — `/goal` command with structured stop conditions.
//!
//! This module implements the goal-based execution pattern: user states a goal,
//! LLM translates it into structured check conditions, and a sub-agent
//! autonomously iterates until all conditions pass or guardrails trip.
//!
//! # Architecture
//!
//! - [`condition`] — [`GoalCondition`](condition::GoalCondition) enum with
//!   deterministic `check()` execution (exit code, file exists, numeric, etc.)
//! - [`plan`] — [`GoalPlan`](plan::GoalPlan) with parsed conditions, max rounds
//! - [`event`] — [`GoalEvent`](event::GoalEvent) emitted during execution
//! - [`runner`] — [`GoalRunner`](runner::GoalRunner) background execution loop

pub mod condition;
pub mod event;
pub mod persist;
pub mod plan;
pub mod runner;

pub use condition::{CheckResult, Comparison, GoalCondition};
pub use event::{BlockedReason, BlockedReasonCode, GoalEvent};
pub use plan::GoalPlan;
pub use runner::GoalRunner;
