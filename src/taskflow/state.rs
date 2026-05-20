//! TaskFlow execution state machine and checkpoint types
//!
//! Defines the state machine for durable task execution with
//! checkpoint/resume capabilities.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Execution state of a TaskFlow
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskFlowState {
    /// Ready to start but not yet running
    Idle,
    /// Currently executing
    Running,
    /// Paused by user or system
    Paused,
    /// Failed with an error
    Failed,
    /// All tasks completed successfully
    Completed,
    /// Recovering from a checkpoint
    Recovering,
}

impl std::fmt::Display for TaskFlowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskFlowState::Idle => write!(f, "idle"),
            TaskFlowState::Running => write!(f, "running"),
            TaskFlowState::Paused => write!(f, "paused"),
            TaskFlowState::Failed => write!(f, "failed"),
            TaskFlowState::Completed => write!(f, "completed"),
            TaskFlowState::Recovering => write!(f, "recovering"),
        }
    }
}

/// A checkpoint captures the full execution state at a point in time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFlowCheckpoint {
    /// Unique checkpoint ID
    pub id: String,
    /// Flow / execution ID
    pub flow_id: String,
    /// Current execution state
    pub state: TaskFlowState,
    /// Current task index within the plan
    pub current_task_index: usize,
    /// IDs of completed tasks
    pub completed_tasks: Vec<String>,
    /// Task outputs keyed by task ID
    pub task_outputs: HashMap<String, String>,
    /// Shared variables across the flow
    pub variables: HashMap<String, String>,
    /// Retry count for current task
    pub retry_count: u32,
    /// Error message if state is Failed
    pub error: Option<String>,
    /// When this checkpoint was created
    pub created_at: DateTime<Utc>,
    /// Original user request / goal
    pub goal: String,
    /// Serialized plan (JSON)
    pub plan_json: String,
    /// Checkpoint sequence number (monotonically increasing)
    pub sequence: u64,
}

impl TaskFlowCheckpoint {
    /// Create a new checkpoint for a flow
    pub fn new(flow_id: impl Into<String>, goal: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            flow_id: flow_id.into(),
            state: TaskFlowState::Idle,
            current_task_index: 0,
            completed_tasks: Vec::new(),
            task_outputs: HashMap::new(),
            variables: HashMap::new(),
            retry_count: 0,
            error: None,
            created_at: Utc::now(),
            goal: goal.into(),
            plan_json: String::new(),
            sequence: 0,
        }
    }

    /// Mark as running
    pub fn mark_running(mut self) -> Self {
        self.state = TaskFlowState::Running;
        self
    }

    /// Mark current task as complete and advance
    pub fn complete_task(&mut self, task_id: impl Into<String>, output: impl Into<String>) {
        let id = task_id.into();
        self.completed_tasks.push(id.clone());
        self.task_outputs.insert(id, output.into());
        self.current_task_index += 1;
        self.retry_count = 0;
    }

    /// Record a task failure
    pub fn record_failure(&mut self, error: impl Into<String>) {
        self.state = TaskFlowState::Failed;
        self.error = Some(error.into());
    }

    /// Mark as paused
    pub fn mark_paused(&mut self) {
        self.state = TaskFlowState::Paused;
    }

    /// Mark as completed
    pub fn mark_completed(&mut self) {
        self.state = TaskFlowState::Completed;
    }

    /// Set a shared variable
    pub fn set_variable(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.variables.insert(key.into(), value.into());
    }

    /// Get a shared variable
    pub fn get_variable(&self, key: &str) -> Option<&String> {
        self.variables.get(key)
    }

    /// Increment retry counter
    pub fn increment_retry(&mut self) {
        self.retry_count += 1;
    }

    /// Check if max retries exceeded
    pub fn max_retries_exceeded(&self, max_retries: u32) -> bool {
        self.retry_count >= max_retries
    }

    /// Create a successor checkpoint (increments sequence)
    pub fn successor(&self) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            flow_id: self.flow_id.clone(),
            state: self.state,
            current_task_index: self.current_task_index,
            completed_tasks: self.completed_tasks.clone(),
            task_outputs: self.task_outputs.clone(),
            variables: self.variables.clone(),
            retry_count: self.retry_count,
            error: self.error.clone(),
            created_at: Utc::now(),
            goal: self.goal.clone(),
            plan_json: self.plan_json.clone(),
            sequence: self.sequence + 1,
        }
    }
}

/// Configuration for TaskFlow execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFlowConfig {
    /// Maximum retries per task
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Delay between retries in seconds
    #[serde(default = "default_retry_delay_secs")]
    pub retry_delay_secs: u64,
    /// Whether to checkpoint after each task
    #[serde(default = "default_checkpoint_after_each_task")]
    pub checkpoint_after_each_task: bool,
    /// Whether to auto-resume from last checkpoint on start
    #[serde(default = "default_auto_resume")]
    pub auto_resume: bool,
    /// Maximum age of checkpoint to auto-resume (seconds)
    #[serde(default = "default_max_checkpoint_age_secs")]
    pub max_checkpoint_age_secs: u64,
}

impl Default for TaskFlowConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            retry_delay_secs: default_retry_delay_secs(),
            checkpoint_after_each_task: default_checkpoint_after_each_task(),
            auto_resume: default_auto_resume(),
            max_checkpoint_age_secs: default_max_checkpoint_age_secs(),
        }
    }
}

fn default_max_retries() -> u32 {
    3
}
fn default_retry_delay_secs() -> u64 {
    5
}
fn default_checkpoint_after_each_task() -> bool {
    true
}
fn default_auto_resume() -> bool {
    true
}
fn default_max_checkpoint_age_secs() -> u64 {
    86400
} // 24 hours

/// Summary of a TaskFlow execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFlowSummary {
    pub flow_id: String,
    pub state: TaskFlowState,
    pub current_task: usize,
    pub total_tasks: usize,
    pub completed_tasks: usize,
    pub retry_count: u32,
    pub error: Option<String>,
    pub last_checkpoint_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_new() {
        let cp = TaskFlowCheckpoint::new("flow-1", "Build app");
        assert_eq!(cp.flow_id, "flow-1");
        assert_eq!(cp.goal, "Build app");
        assert_eq!(cp.state, TaskFlowState::Idle);
        assert_eq!(cp.current_task_index, 0);
        assert_eq!(cp.sequence, 0);
    }

    #[test]
    fn test_checkpoint_complete_task() {
        let mut cp = TaskFlowCheckpoint::new("f", "g");
        cp.complete_task("task_1", "output_1");
        assert_eq!(cp.current_task_index, 1);
        assert_eq!(cp.completed_tasks, vec!["task_1"]);
        assert_eq!(cp.task_outputs.get("task_1"), Some(&"output_1".to_string()));
        assert_eq!(cp.retry_count, 0);
    }

    #[test]
    fn test_checkpoint_mark_paused() {
        let mut cp = TaskFlowCheckpoint::new("f", "g").mark_running();
        cp.mark_paused();
        assert_eq!(cp.state, TaskFlowState::Paused);
    }

    #[test]
    fn test_checkpoint_record_failure() {
        let mut cp = TaskFlowCheckpoint::new("f", "g");
        cp.record_failure("Something went wrong");
        assert_eq!(cp.state, TaskFlowState::Failed);
        assert_eq!(cp.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_checkpoint_variables() {
        let mut cp = TaskFlowCheckpoint::new("f", "g");
        cp.set_variable("key", "value");
        assert_eq!(cp.get_variable("key"), Some(&"value".to_string()));
        assert_eq!(cp.get_variable("missing"), None);
    }

    #[test]
    fn test_checkpoint_successor() {
        let mut cp = TaskFlowCheckpoint::new("f", "g");
        cp.complete_task("t1", "done");
        cp.set_variable("x", "y");

        let next = cp.successor();
        assert_eq!(next.sequence, 1);
        assert_eq!(next.current_task_index, 1);
        assert_eq!(next.variables.get("x"), Some(&"y".to_string()));
        assert_ne!(next.id, cp.id);
    }

    #[test]
    fn test_checkpoint_max_retries() {
        let mut cp = TaskFlowCheckpoint::new("f", "g");
        assert!(!cp.max_retries_exceeded(3));
        cp.retry_count = 3;
        assert!(cp.max_retries_exceeded(3));
    }

    #[test]
    fn test_task_flow_config_default() {
        let config = TaskFlowConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.retry_delay_secs, 5);
        assert!(config.checkpoint_after_each_task);
        assert!(config.auto_resume);
        assert_eq!(config.max_checkpoint_age_secs, 86400);
    }

    #[test]
    fn test_task_flow_state_display() {
        assert_eq!(TaskFlowState::Running.to_string(), "running");
        assert_eq!(TaskFlowState::Paused.to_string(), "paused");
        assert_eq!(TaskFlowState::Failed.to_string(), "failed");
    }

    #[test]
    fn test_task_flow_summary_serde() {
        let summary = TaskFlowSummary {
            flow_id: "f".to_string(),
            state: TaskFlowState::Running,
            current_task: 2,
            total_tasks: 5,
            completed_tasks: 2,
            retry_count: 1,
            error: None,
            last_checkpoint_at: Some(Utc::now()),
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("running"));
        let restored: TaskFlowSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.current_task, 2);
        assert_eq!(restored.total_tasks, 5);
    }
}
