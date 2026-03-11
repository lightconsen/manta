//! Todo Tool - Task management for the agent
//!
//! This tool allows the agent to create, update, and manage tasks
//! during complex multi-step operations.

use super::{Tool, ToolContext, ToolExecutionResult};
use crate::agent::todo::{TaskStatus, TodoStore};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Tool for managing tasks/todos
#[derive(Debug)]
pub struct TodoTool {
    /// In-memory storage of todo lists per conversation
    stores: Arc<RwLock<HashMap<String, TodoStore>>>,
}

impl TodoTool {
    /// Create a new todo tool
    pub fn new() -> Self {
        Self {
            stores: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create a store for a conversation
    async fn get_store(&self, conversation_id: &str) -> TodoStore {
        let stores = self.stores.read().await;
        if let Some(store) = stores.get(conversation_id) {
            return store.clone();
        }
        drop(stores);

        let mut stores = self.stores.write().await;
        stores
            .entry(conversation_id.to_string())
            .or_insert_with(TodoStore::new)
            .clone()
    }

    /// Save a store for a conversation
    async fn save_store(&self, conversation_id: &str, store: TodoStore) {
        let mut stores = self.stores.write().await;
        stores.insert(conversation_id.to_string(), store);
    }
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        r#"Manage tasks and todo lists.

Use this tool for complex tasks with 3+ steps to track progress and ensure completion.
You can create tasks, update their status, list active tasks, and clear completed ones.

Examples:
- Create tasks for a multi-step project
- Mark tasks as complete when done
- List pending tasks to see what's remaining
- Clear completed tasks when finished"#
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "update", "list", "clear_completed", "get_status"],
                    "description": "The action to perform"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID (required for update action)"
                },
                "content": {
                    "type": "string",
                    "description": "Task content/description (required for create action)"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "cancelled"],
                    "description": "New status (for update action)"
                },
                "priority": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 5,
                    "description": "Task priority 1-5 (1=highest, 5=lowest)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("action is required".to_string()))?;

        let conversation_id = &context.conversation_id;
        let mut store = self.get_store(conversation_id).await;

        match action {
            "create" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("content is required for create".to_string()))?;

                let mut task = store.create_task(content);

                if let Some(priority) = args["priority"].as_u64() {
                    task.set_priority(priority as u8);
                }

                self.save_store(conversation_id, store).await;

                Ok(ToolExecutionResult::success(format!("Created task: {}", task.content))
                    .with_data(json!({
                        "task_id": task.id,
                        "content": task.content,
                        "status": task.status.to_string(),
                        "priority": task.priority
                    })))
            }

            "update" => {
                let task_id = args["task_id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("task_id is required for update".to_string()))?;

                let task = store
                    .get_mut(task_id)
                    .ok_or_else(|| crate::error::MantaError::Validation(format!("Task {} not found", task_id)))?;

                if let Some(status_str) = args["status"].as_str() {
                    let status = match status_str {
                        "pending" => TaskStatus::Pending,
                        "in_progress" => TaskStatus::InProgress,
                        "completed" => TaskStatus::Completed,
                        "cancelled" => TaskStatus::Cancelled,
                        _ => {
                            return Err(crate::error::MantaError::Validation(format!(
                                "Invalid status: {}",
                                status_str
                            )))
                        }
                    };
                    task.set_status(status);
                }

                if let Some(priority) = args["priority"].as_u64() {
                    task.set_priority(priority as u8);
                }

                let task_clone = task.clone();
                self.save_store(conversation_id, store).await;

                Ok(ToolExecutionResult::success(format!("Updated task: {}", task_clone.content))
                    .with_data(json!({
                        "task_id": task_clone.id,
                        "content": task_clone.content,
                        "status": task_clone.status.to_string(),
                        "priority": task_clone.priority
                    })))
            }

            "list" => {
                let tasks: Vec<_> = store
                    .list()
                    .into_iter()
                    .map(|t| {
                        json!({
                            "id": t.id,
                            "content": t.content,
                            "status": t.status.to_string(),
                            "priority": t.priority,
                            "created_at": t.created_at.to_rfc3339()
                        })
                    })
                    .collect();

                let active_count = store.list_active().len();
                let formatted = store.format_for_prompt();

                Ok(ToolExecutionResult::success(formatted)
                    .with_data(json!({
                        "tasks": tasks,
                        "total": store.count(),
                        "active": active_count
                    })))
            }

            "clear_completed" => {
                let cleared = store.clear_completed();
                self.save_store(conversation_id, store).await;

                Ok(ToolExecutionResult::success(format!("Cleared {} completed tasks", cleared))
                    .with_data(json!({"cleared": cleared})))
            }

            "get_status" => {
                let total = store.count();
                let pending = store.count_by_status(TaskStatus::Pending);
                let in_progress = store.count_by_status(TaskStatus::InProgress);
                let completed = store.count_by_status(TaskStatus::Completed);

                let summary = format!(
                    "Tasks: {} total, {} pending, {} in progress, {} completed",
                    total, pending, in_progress, completed
                );

                Ok(ToolExecutionResult::success(summary)
                    .with_data(json!({
                        "total": total,
                        "pending": pending,
                        "in_progress": in_progress,
                        "completed": completed
                    })))
            }

            _ => Err(crate::error::MantaError::Validation(format!(
                "Unknown action: {}",
                action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_todo_tool_create() {
        let tool = TodoTool::new();
        let ctx = ToolContext::new("user", "conv_1");

        let args = json!({
            "action": "create",
            "content": "Test task"
        });

        let result = tool.execute(args, &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_todo_tool_list() {
        let tool = TodoTool::new();
        let ctx = ToolContext::new("user", "conv_1");

        // Create a task first
        tool.execute(
            json!({"action": "create", "content": "Task 1"}),
            &ctx,
        )
        .await
        .unwrap();

        let result = tool
            .execute(json!({"action": "list"}), &ctx)
            .await
            .unwrap();

        let output = result.to_string();
        assert!(output.contains("Task 1"));
    }
}
