//! Goal planner — decompose high-level goals into executable task DAGs.
//!
//! The planner takes a user goal (e.g. "deploy this project to a server"),
//! breaks it into a directed acyclic graph of [`Task`]s, and executes them
//! in topological order with automatic verification and rollback.

use crate::computer::{ComputerAdapter, DesktopAction, VerificationCriteria};
use crate::computer::VerificationEngine;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub mod dag;
pub mod executor;
pub mod workflow;

pub use dag::DagScheduler;
pub use executor::TaskExecutor;
pub use workflow::{FailureStrategy, RecordedStep, StepResult, Workflow, WorkflowAction, WorkflowPlayer, WorkflowRecorder};

/// Unique identifier for a task.
pub type TaskId = String;

/// Current state of a task in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// Waiting for dependencies to complete.
    Pending,
    /// Currently executing.
    Running,
    /// Completed successfully.
    Completed,
    /// Failed after all retries exhausted.
    Failed,
    /// Rolled back due to downstream failure.
    RolledBack,
}

/// A single unit of work in a goal plan.
#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub description: String,
    /// The desktop action to execute.
    pub action: DesktopAction,
    /// Task IDs that must complete before this one can start.
    pub dependencies: Vec<TaskId>,
    /// How to verify this task succeeded.
    pub verification: Option<VerificationCriteria>,
    /// Whether to snapshot before executing (enables rollback).
    pub snapshot_before: bool,
    /// Number of retry attempts on failure (0 = no retries).
    pub max_retries: u32,
    /// Delay between retries.
    pub retry_delay: Duration,
    /// Status (managed by the executor).
    pub status: TaskStatus,
    /// Error message if the task failed.
    pub error: Option<String>,
    /// Result message if the task succeeded.
    pub result: Option<String>,
}

impl Task {
    pub fn new(id: impl Into<String>, description: impl Into<String>, action: DesktopAction) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            action,
            dependencies: vec![],
            verification: None,
            snapshot_before: false,
            max_retries: 2,
            retry_delay: Duration::from_secs(1),
            status: TaskStatus::Pending,
            error: None,
            result: None,
        }
    }

    pub fn depends_on(mut self, id: impl Into<String>) -> Self {
        self.dependencies.push(id.into());
        self
    }

    pub fn with_verification(mut self, criteria: VerificationCriteria) -> Self {
        self.verification = Some(criteria);
        self
    }

    pub fn with_snapshot(mut self) -> Self {
        self.snapshot_before = true;
        self
    }

    pub fn with_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

/// A plan is a collection of tasks with their dependency graph.
#[derive(Debug, Default)]
pub struct Plan {
    pub goal: String,
    pub tasks: HashMap<TaskId, Task>,
}

impl Plan {
    pub fn new(goal: impl Into<String>) -> Self {
        Self {
            goal: goal.into(),
            tasks: HashMap::new(),
        }
    }

    pub fn add_task(&mut self, task: Task) -> &mut Self {
        self.tasks.insert(task.id.clone(), task);
        self
    }

    pub fn get_task(&self, id: &str) -> Option<&Task> {
        self.tasks.get(id)
    }

    pub fn get_task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.get_mut(id)
    }

    /// Returns true if all tasks are completed.
    pub fn is_complete(&self) -> bool {
        self.tasks.values().all(|t| matches!(t.status, TaskStatus::Completed))
    }

    /// Returns true if any task failed.
    pub fn has_failures(&self) -> bool {
        self.tasks.values().any(|t| matches!(t.status, TaskStatus::Failed))
    }

    /// Tasks that are ready to run (all dependencies completed).
    pub fn ready_tasks(&self) -> Vec<TaskId> {
        self.tasks
            .values()
            .filter(|t| {
                t.status == TaskStatus::Pending
                    && t.dependencies.iter().all(|dep| {
                        self.tasks
                            .get(dep)
                            .map(|d| d.status == TaskStatus::Completed)
                            .unwrap_or(false)
                    })
            })
            .map(|t| t.id.clone())
            .collect()
    }

    /// Update a task's status.
    pub fn set_status(&mut self, id: &str, status: TaskStatus) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.status = status;
        }
    }

    /// Mark a task as completed with a result message.
    pub fn complete_task(&mut self, id: &str, result: String) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.status = TaskStatus::Completed;
            task.result = Some(result);
        }
    }

    /// Mark a task as failed with an error message.
    pub fn fail_task(&mut self, id: &str, error: String) {
        if let Some(task) = self.tasks.get_mut(id) {
            task.status = TaskStatus::Failed;
            task.error = Some(error);
        }
    }
}

/// Result of executing a plan.
#[derive(Debug, Clone)]
pub struct PlanResult {
    pub success: bool,
    pub goal: String,
    pub tasks_completed: usize,
    pub tasks_failed: usize,
    pub tasks_rolled_back: usize,
    pub message: String,
}

/// High-level planner that decomposes goals and executes plans.
pub struct GoalPlanner {
    #[allow(dead_code)]
    adapter: Arc<dyn ComputerAdapter>,
    #[allow(dead_code)]
    verifier: VerificationEngine,
    executor: TaskExecutor,
}

impl GoalPlanner {
    pub fn new(adapter: Arc<dyn ComputerAdapter>) -> Self {
        let verifier = VerificationEngine::new(adapter.clone());
        let executor = TaskExecutor::new(adapter.clone(), verifier.clone());
        Self {
            adapter,
            verifier,
            executor,
        }
    }

    /// Execute a pre-built plan.
    pub async fn execute_plan(&self, plan: &mut Plan) -> crate::Result<PlanResult> {
        self.executor.execute(plan).await
    }

    /// Quick helper: execute a linear sequence of tasks (no branching).
    pub async fn execute_sequence(
        &self,
        goal: &str,
        tasks: Vec<Task>,
    ) -> crate::Result<PlanResult> {
        let mut plan = Plan::new(goal);
        for task in tasks {
            plan.add_task(task);
        }
        self.execute_plan(&mut plan).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::{ClickTarget, MouseButton, Point};

    #[test]
    fn test_task_builder() {
        let task = Task::new(
            "click_ok",
            "Click the OK button",
            DesktopAction::Click {
                target: ClickTarget::Coordinate(Point::new(100, 200)),
                button: MouseButton::Left,
            },
        )
        .depends_on("open_dialog")
        .with_retries(3);

        assert_eq!(task.id, "click_ok");
        assert_eq!(task.dependencies, vec!["open_dialog"]);
        assert_eq!(task.max_retries, 3);
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[test]
    fn test_plan_ready_tasks() {
        let mut plan = Plan::new("test goal");
        plan.add_task(Task::new("a", "step A", DesktopAction::Wait { milliseconds: 10 }));
        plan.add_task(
            Task::new("b", "step B", DesktopAction::Wait { milliseconds: 10 }).depends_on("a"),
        );
        plan.add_task(
            Task::new("c", "step C", DesktopAction::Wait { milliseconds: 10 }).depends_on("a"),
        );

        // Initially only 'a' is ready (no deps).
        let ready = plan.ready_tasks();
        assert_eq!(ready, vec!["a"]);

        // After 'a' completes, 'b' and 'c' are ready.
        plan.complete_task("a", "done".to_string());
        let mut ready = plan.ready_tasks();
        ready.sort();
        assert_eq!(ready, vec!["b", "c"]);
    }

    #[test]
    fn test_plan_is_complete() {
        let mut plan = Plan::new("test");
        plan.add_task(Task::new("a", "step A", DesktopAction::Wait { milliseconds: 10 }));

        assert!(!plan.is_complete());
        plan.complete_task("a", "done".to_string());
        assert!(plan.is_complete());
    }

    #[test]
    fn test_plan_has_failures() {
        let mut plan = Plan::new("test");
        plan.add_task(Task::new("a", "step A", DesktopAction::Wait { milliseconds: 10 }));

        assert!(!plan.has_failures());
        plan.fail_task("a", "oops".to_string());
        assert!(plan.has_failures());
    }
}
