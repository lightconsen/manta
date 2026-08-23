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
//! - [`handoff`] — bounded structured handoff for the fresh-context ("Ralph")
//!   loop mode, where each round runs in a brand-new seedless sub-agent

pub mod condition;
pub mod event;
pub mod handoff;
pub mod persist;
pub mod plan;
pub mod runner;

pub use condition::{CheckResult, Comparison, GoalCondition};
pub use event::{BlockedReason, BlockedReasonCode, GoalEvent};
pub use handoff::{extract_handoff, HandoffStatus, RoundHandoff};
pub use plan::GoalPlan;
pub use runner::GoalRunner;
