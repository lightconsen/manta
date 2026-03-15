//! Todo Tool - Task management for the agent
//!
//! This tool allows the agent to create, update, and manage tasks
//! during complex multi-step operations.
//!
//! Tasks are persisted to disk in ~/.manta/todos/{conversation_id}.json

use super::{Tool, ToolContext, ToolExecutionResult};
use crate::agent::todo::{TaskStatus, TodoStore};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Tool for managing tasks/todos
#[derive(Debug)]
pub struct TodoTool {
    /// In-memory storage of todo lists per conversation
    stores: Arc<RwLock<HashMap<String, TodoStore>>>,
    /// Base directory for todo files
    base_dir: PathBuf,
}

impl TodoTool {
    /// Create a new todo tool
    pub fn new() -> Self {
        Self {
            stores: Arc::new(RwLock::new(HashMap::new())),
            base_dir: crate::dirs::todos_dir(),
        }
    }

    /// Create with custom directory (for testing)
    #[allow(dead_code)]
    pub fn with_dir(base_dir: PathBuf) -> Self {
        Self {
            stores: Arc::new(RwLock::new(HashMap::new())),
            base_dir,
        }
    }

    /// Get the file path for a conversation's todo file
    fn todo_file_path(&self, conversation_id: &str) -> PathBuf {
        // Sanitize conversation ID to be safe for filenames
        let safe_id = conversation_id.replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "_");
        self.base_dir.join(format!("{}.json", safe_id))
    }

    /// Load a todo store from disk
    async fn load_from_disk(&self, conversation_id: &str) -> Option<TodoStore> {
        let path = self.todo_file_path(conversation_id);

        if !path.exists() {
            return None;
        }

        debug!("Loading todo store from {:?}", path);

        match tokio::fs::read_to_string(&path).await {
            Ok(content) => {
                match TodoStore::from_json(&content) {
                    Ok(store) => {
                        debug!("Loaded {} tasks for conversation {}", store.count(), conversation_id);
                        Some(store)
                    }
                    Err(e) => {
                        error!("Failed to parse todo file {:?}: {}", path, e);
                        None
                    }
                }
            }
            Err(e) => {
                error!("Failed to read todo file {:?}: {}", path, e);
                None
            }
        }
    }

    /// Save a todo store to disk
    async fn save_to_disk(&self, conversation_id: &str, store: &TodoStore) {
        let path = self.todo_file_path(conversation_id);

        debug!("Saving todo store to {:?}", path);

        match store.to_json() {
            Ok(json) => {
                if let Err(e) = tokio::fs::write(&path, json).await {
                    error!("Failed to write todo file {:?}: {}", path, e);
                } else {
                    debug!("Saved {} tasks for conversation {}", store.count(), conversation_id);
                }
            }
            Err(e) => {
                error!("Failed to serialize todo store: {}", e);
            }
        }
    }

    /// Get or create a store for a conversation
    async fn get_store(&self, conversation_id: &str) -> TodoStore {
        // First check in-memory cache
        {
            let stores = self.stores.read().await;
            if let Some(store) = stores.get(conversation_id) {
                return store.clone();
            }
        }

        // Try to load from disk
        if let Some(store) = self.load_from_disk(conversation_id).await {
            let mut stores = self.stores.write().await;
            stores.insert(conversation_id.to_string(), store.clone());
            return store;
        }

        // Create new store
        let store = TodoStore::new();
        let mut stores = self.stores.write().await;
        stores.insert(conversation_id.to_string(), store.clone());
        store
    }

    /// Save a store for a conversation (memory + disk)
    async fn save_store(&self, conversation_id: &str, store: TodoStore) {
        // Save to disk first
        self.save_to_disk(conversation_id, &store).await;

        // Update in-memory cache
        let mut stores = self.stores.write().await;
        stores.insert(conversation_id.to_string(), store);
    }

    /// Clean up old completed tasks across all conversations
    /// Returns number of tasks cleaned up
    pub async fn cleanup_old_completed(&self, max_age_days: i64) -> usize {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days);
        let mut total_cleaned = 0;

        // Get list of all todo files
        let mut entries = match tokio::fs::read_dir(&self.base_dir).await {
            Ok(entries) => entries,
            Err(e) => {
                error!("Failed to read todos directory: {}", e);
                return 0;
            }
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            let conversation_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            if let Some(mut store) = self.load_from_disk(&conversation_id).await {
                let before_count = store.count();

                // Remove old completed tasks
                let old_completed: Vec<String> = store
                    .list()
                    .into_iter()
                    .filter(|t| {
                        t.status == TaskStatus::Completed
                            && t.completed_at.map(|t| t < cutoff).unwrap_or(false)
                    })
                    .map(|t| t.id.clone())
                    .collect();

                for task_id in old_completed {
                    store.remove(&task_id);
                    total_cleaned += 1;
                }

                // If store is empty, delete the file
                if store.count() == 0 {
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        warn!("Failed to remove empty todo file {:?}: {}", path, e);
                    } else {
                        debug!("Removed empty todo file {:?}", path);
                    }
                } else if store.count() != before_count {
                    // Save if we removed some tasks
                    self.save_to_disk(&conversation_id, &store).await;

                    // Update cache if present
                    let mut stores = self.stores.write().await;
                    if stores.contains_key(&conversation_id) {
                        stores.insert(conversation_id, store);
                    }
                }
            }
        }

        if total_cleaned > 0 {
            info!("Cleaned up {} old completed tasks", total_cleaned);
        }

        total_cleaned
    }

    /// List all conversations with todos
    pub async fn list_conversations(&self) -> Vec<String> {
        let mut conversations = Vec::new();

        let mut entries = match tokio::fs::read_dir(&self.base_dir).await {
            Ok(entries) => entries,
            Err(_) => return conversations,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    conversations.push(stem.to_string());
                }
            }
        }

        conversations
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

Tasks are automatically saved and persist across daemon restarts.

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
        let temp_dir = std::env::temp_dir().join(format!("manta_todo_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let tool = TodoTool::with_dir(temp_dir.clone());
        let ctx = ToolContext::new("user", "conv_1");

        let args = json!({
            "action": "create",
            "content": "Test task"
        });

        let result = tool.execute(args, &ctx).await;
        assert!(result.is_ok());

        // Verify file was created
        let todo_file = temp_dir.join("conv_1.json");
        assert!(todo_file.exists());

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_todo_tool_persistence() {
        let temp_dir = std::env::temp_dir().join(format!("manta_todo_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        // Create first tool instance and add task
        {
            let tool = TodoTool::with_dir(temp_dir.clone());
            let ctx = ToolContext::new("user", "persistent_conv");

            tool.execute(
                json!({"action": "create", "content": "Persistent task"}),
                &ctx,
            )
            .await
            .unwrap();
        }

        // Create second tool instance (simulating daemon restart)
        {
            let tool = TodoTool::with_dir(temp_dir.clone());
            let ctx = ToolContext::new("user", "persistent_conv");

            let result = tool
                .execute(json!({"action": "list"}), &ctx)
                .await
                .unwrap();

            let output = result.to_string();
            assert!(output.contains("Persistent task"));
        }

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
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

    #[tokio::test]
    async fn test_todo_cleanup() {
        let temp_dir = std::env::temp_dir().join(format!("manta_todo_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let tool = TodoTool::with_dir(temp_dir.clone());

        // Create a task and complete it
        {
            let ctx = ToolContext::new("user", "cleanup_conv");
            tool.execute(
                json!({"action": "create", "content": "Old task"}),
                &ctx,
            )
            .await
            .unwrap();

            tool.execute(
                json!({"action": "update", "task_id": "task_1", "status": "completed"}),
                &ctx,
            )
            .await
            .unwrap();
        }

        // Manually modify the file to make the task old
        // Create JSON with an old completed_at date directly
        let todo_file = temp_dir.join("cleanup_conv.json");
        let old_date = (chrono::Utc::now() - chrono::Duration::days(40)).to_rfc3339();
        let modified = format!(
            r#"{{"tasks":{{"task_1":{{"id":"task_1","content":"Old task","status":"completed","created_at":"{}","updated_at":"{}","completed_at":"{}","parent_id":null,"subtasks":[],"priority":3,"metadata":{{}}}}}},"order":["task_1"]}}"#,
            old_date, old_date, old_date
        );
        tokio::fs::write(&todo_file, modified).await.unwrap();

        // Run cleanup (30 days)
        let cleaned = tool.cleanup_old_completed(30).await;
        assert_eq!(cleaned, 1);

        // Verify file was removed (since it was empty after cleanup)
        assert!(!todo_file.exists());

        // Clean up
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
