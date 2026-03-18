//! Task Planning System (Todo) for Manta
//!
//! This module implements a task management system that allows the agent
//! to track and manage complex multi-step tasks.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Status of a task
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Task is pending and not yet started
    #[serde(rename = "pending")]
    Pending,
    /// Task is currently in progress
    #[serde(rename = "in_progress")]
    InProgress,
    /// Task has been completed
    #[serde(rename = "completed")]
    Completed,
    /// Task was cancelled
    #[serde(rename = "cancelled")]
    Cancelled,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::InProgress => write!(f, "in_progress"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// A single task in the todo system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task ID
    pub id: String,
    /// Task content/description
    pub content: String,
    /// Current status
    pub status: TaskStatus,
    /// When the task was created
    pub created_at: DateTime<Utc>,
    /// When the task was last updated
    pub updated_at: DateTime<Utc>,
    /// When the task was completed (if applicable)
    pub completed_at: Option<DateTime<Utc>>,
    /// Optional parent task ID for subtasks
    pub parent_id: Option<String>,
    /// Subtask IDs
    pub subtasks: Vec<String>,
    /// Task priority (1-5, lower is higher priority)
    pub priority: u8,
    /// Additional metadata
    #[serde(flatten)]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl Task {
    /// Create a new task
    pub fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            content: content.into(),
            status: TaskStatus::Pending,
            created_at: now,
            updated_at: now,
            completed_at: None,
            parent_id: None,
            subtasks: Vec::new(),
            priority: 3, // Default medium priority
            metadata: HashMap::new(),
        }
    }

    /// Set the task status
    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
        self.updated_at = Utc::now();
        if status == TaskStatus::Completed {
            self.completed_at = Some(Utc::now());
        }
    }

    /// Mark task as in progress
    pub fn start(&mut self) {
        self.set_status(TaskStatus::InProgress);
    }

    /// Mark task as completed
    pub fn complete(&mut self) {
        self.set_status(TaskStatus::Completed);
    }

    /// Mark task as cancelled
    pub fn cancel(&mut self) {
        self.set_status(TaskStatus::Cancelled);
    }

    /// Add a subtask
    pub fn add_subtask(&mut self, subtask_id: impl Into<String>) {
        self.subtasks.push(subtask_id.into());
        self.updated_at = Utc::now();
    }

    /// Set parent task
    pub fn set_parent(&mut self, parent_id: impl Into<String>) {
        self.parent_id = Some(parent_id.into());
        self.updated_at = Utc::now();
    }

    /// Set priority (1-5)
    pub fn set_priority(&mut self, priority: u8) {
        self.priority = priority.clamp(1, 5);
        self.updated_at = Utc::now();
    }

    /// Add metadata
    pub fn with_metadata(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Check if task is active (pending or in_progress)
    pub fn is_active(&self) -> bool {
        matches!(self.status, TaskStatus::Pending | TaskStatus::InProgress)
    }

    /// Get task summary for display
    pub fn summary(&self) -> String {
        let status_icon = match self.status {
            TaskStatus::Pending => "⏳",
            TaskStatus::InProgress => "🔄",
            TaskStatus::Completed => "✅",
            TaskStatus::Cancelled => "❌",
        };
        format!("{} [{}] {}", status_icon, self.id, self.content)
    }
}

/// Store for managing tasks
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoStore {
    /// Tasks stored by ID
    tasks: HashMap<String, Task>,
    /// Task order for display
    order: Vec<String>,
}

impl TodoStore {
    /// Create a new empty todo store
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// Create a new task with auto-generated ID
    pub fn create_task(&mut self, content: impl Into<String>) -> Task {
        let id = format!("task_{}", self.tasks.len() + 1);
        let task = Task::new(&id, content);
        self.tasks.insert(id.clone(), task.clone());
        self.order.push(id);
        task
    }

    /// Create a task with specific ID
    pub fn create_task_with_id(
        &mut self,
        id: impl Into<String>,
        content: impl Into<String>,
    ) -> Task {
        let id = id.into();
        let task = Task::new(&id, content);
        self.tasks.insert(id.clone(), task.clone());
        self.order.push(id);
        task
    }

    /// Get a task by ID
    pub fn get(&self, id: &str) -> Option<&Task> {
        self.tasks.get(id)
    }

    /// Get a mutable task by ID
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.get_mut(id)
    }

    /// Update a task
    pub fn update(&mut self, task: Task) -> Option<Task> {
        if self.tasks.contains_key(&task.id) {
            self.tasks.insert(task.id.clone(), task)
        } else {
            None
        }
    }

    /// Remove a task
    pub fn remove(&mut self, id: &str) -> Option<Task> {
        self.order.retain(|x| x != id);
        self.tasks.remove(id)
    }

    /// List all tasks
    pub fn list(&self) -> Vec<&Task> {
        self.order
            .iter()
            .filter_map(|id| self.tasks.get(id))
            .collect()
    }

    /// List tasks with specific status
    pub fn list_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.tasks.values().filter(|t| t.status == status).collect()
    }

    /// List active tasks (pending or in_progress)
    pub fn list_active(&self) -> Vec<&Task> {
        self.tasks.values().filter(|t| t.is_active()).collect()
    }

    /// Get count of tasks by status
    pub fn count_by_status(&self, status: TaskStatus) -> usize {
        self.tasks.values().filter(|t| t.status == status).count()
    }

    /// Get total task count
    pub fn count(&self) -> usize {
        self.tasks.len()
    }

    /// Clear all completed tasks
    pub fn clear_completed(&mut self) -> usize {
        let completed: Vec<String> = self
            .tasks
            .values()
            .filter(|t| t.status == TaskStatus::Completed)
            .map(|t| t.id.clone())
            .collect();
        let count = completed.len();
        for id in completed {
            self.remove(&id);
        }
        count
    }

    /// Format tasks for display in system prompt
    pub fn format_for_prompt(&self) -> String {
        if self.tasks.is_empty() {
            return "No active tasks.".to_string();
        }

        let mut lines = vec!["Current Tasks:".to_string()];
        for task in self.list() {
            let indent = if task.parent_id.is_some() { "  " } else { "" };
            lines.push(format!("{}{}", indent, task.summary()));
        }
        lines.join("\n")
    }

    /// Serialize to JSON
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| crate::error::MantaError::Serialization(e))
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> crate::Result<Self> {
        serde_json::from_str(json).map_err(|e| crate::error::MantaError::Serialization(e))
    }
}

/// Tool context for todo operations
#[derive(Debug, Clone)]
pub struct TodoContext {
    /// The store to operate on
    pub store: TodoStore,
    /// Conversation ID this todo list belongs to
    pub conversation_id: String,
}

impl TodoContext {
    /// Create a new todo context
    pub fn new(conversation_id: impl Into<String>) -> Self {
        Self {
            store: TodoStore::new(),
            conversation_id: conversation_id.into(),
        }
    }

    /// Load from JSON
    pub fn from_json(conversation_id: impl Into<String>, json: &str) -> crate::Result<Self> {
        Ok(Self {
            store: TodoStore::from_json(json)?,
            conversation_id: conversation_id.into(),
        })
    }

    /// Save to JSON
    pub fn to_json(&self) -> crate::Result<String> {
        self.store.to_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_creation() {
        let task = Task::new("task_1", "Test task");
        assert_eq!(task.id, "task_1");
        assert_eq!(task.content, "Test task");
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(task.is_active());
    }

    #[test]
    fn test_task_status_transitions() {
        let mut task = Task::new("task_1", "Test task");

        task.start();
        assert_eq!(task.status, TaskStatus::InProgress);

        task.complete();
        assert_eq!(task.status, TaskStatus::Completed);
        assert!(!task.is_active());
        assert!(task.completed_at.is_some());
    }

    #[test]
    fn test_todo_store() {
        let mut store = TodoStore::new();

        let task1 = store.create_task("First task");
        let task2 = store.create_task("Second task");

        assert_eq!(store.count(), 2);
        assert!(store.get(&task1.id).is_some());
        assert!(store.get(&task2.id).is_some());

        let active = store.list_active();
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn test_task_priorities() {
        let mut task = Task::new("task_1", "Test");

        task.set_priority(1);
        assert_eq!(task.priority, 1);

        task.set_priority(10); // Should clamp to 5
        assert_eq!(task.priority, 5);

        task.set_priority(0); // Should clamp to 1
        assert_eq!(task.priority, 1);
    }

    #[test]
    fn test_clear_completed() {
        let mut store = TodoStore::new();

        let mut task1 = store.create_task("Task 1");
        let mut task2 = store.create_task("Task 2");

        if let Some(t) = store.get_mut(&task1.id) {
            t.complete();
        }

        let cleared = store.clear_completed();
        assert_eq!(cleared, 1);
        assert_eq!(store.count(), 1);
    }
}
