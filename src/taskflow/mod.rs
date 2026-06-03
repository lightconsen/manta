//! TaskFlow - Durable execution for multi-step task plans
//!
//! TaskFlow provides checkpoint/resume capabilities for long-running
//! task plans. Execution state is persisted to SQLite, allowing recovery
//! after crashes, pauses, or retries.
//!
//! # Example
//! ```rust,ignore
//! use syscity::taskflow::{TaskFlowEngine, TaskFlowConfig};
//! use syscity::agent::planner::TaskPlan;
//!
//! async fn example() {
//!     let store = CheckpointStore::new("sqlite://taskflow.db").await.unwrap();
//!     let engine = TaskFlowEngine::new(store).await.unwrap();
//!
//!     let plan = TaskPlan::new("Build a CLI tool", "Create a Rust CLI");
//!     // ... add tasks to plan ...
//!
//!     let result = engine.run("build-cli", &plan, executor).await.unwrap();
//! }
//! ```

pub mod engine;
pub mod state;
pub mod store;

pub use engine::{TaskExecutor, TaskFlowContext, TaskFlowEngine, TaskResult, TestExecutor};
pub use state::{TaskFlowCheckpoint, TaskFlowConfig, TaskFlowState, TaskFlowSummary};
pub use store::CheckpointStore;
