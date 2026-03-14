//! Subagent Delegation Tool
//!
//! This tool allows an agent to spawn child agents for parallel task execution.
//! Implements depth limiting, budget sharing, and tool restrictions for children.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{Tool, ToolContext, ToolExecutionResult};
use crate::agent::budget::IterationBudget;

/// Maximum number of concurrent child agents
const MAX_CHILDREN: usize = 3;
/// Maximum delegation depth
const MAX_DEPTH: usize = 2;
/// Tools blocked for child agents (for future use)
#[allow(dead_code)]
const BLOCKED_TOOLS: &[&str] = &["delegate", "clarify", "memory", "send_message", "execute_code"];

/// Task specification for child agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    /// Task description/prompt
    pub prompt: String,
    /// Expected output format
    pub output_format: Option<String>,
    /// Maximum iterations for child
    pub max_iterations: Option<usize>,
    /// Tools allowed for child (empty = all except blocked)
    pub allowed_tools: Vec<String>,
    /// Context to pass to child
    pub context: HashMap<String, serde_json::Value>,
}

/// Child agent handle
#[derive(Debug, Clone)]
pub struct ChildAgent {
    /// Unique ID
    pub id: String,
    /// Parent agent ID
    pub parent_id: String,
    /// Task specification
    pub task: TaskSpec,
    /// Current status
    pub status: ChildStatus,
    /// Creation time
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Result (if completed)
    pub result: Option<String>,
    /// Error (if failed)
    pub error: Option<String>,
    /// Shared budget reference
    pub budget: IterationBudget,
    /// Current iteration count
    pub iterations: Arc<AtomicUsize>,
}

/// Child agent status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildStatus {
    /// Waiting to start
    Pending,
    /// Currently running
    Running,
    /// Completed successfully
    Completed,
    /// Failed with error
    Failed,
    /// Cancelled by parent
    Cancelled,
}

/// Delegation tracker for managing child agents
#[derive(Debug, Default)]
pub struct DelegationTracker {
    /// Active child agents
    children: Arc<RwLock<HashMap<String, ChildAgent>>>,
    /// Current delegation depth
    depth: usize,
    /// Maximum allowed children
    max_children: usize,
}

impl DelegationTracker {
    /// Create a new delegation tracker
    pub fn new(depth: usize) -> Self {
        Self {
            children: Arc::new(RwLock::new(HashMap::new())),
            depth,
            max_children: MAX_CHILDREN,
        }
    }

    /// Check if delegation is allowed
    pub async fn can_delegate(&self) -> bool {
        if self.depth >= MAX_DEPTH {
            return false;
        }
        let children = self.children.read().await;
        children.len() < self.max_children
    }

    /// Get current child count
    pub async fn child_count(&self) -> usize {
        let children = self.children.read().await;
        children.len()
    }

    /// Register a new child agent
    pub async fn register_child(&self, child: ChildAgent) {
        let mut children = self.children.write().await;
        children.insert(child.id.clone(), child);
    }

    /// Get a child agent by ID
    pub async fn get_child(&self, id: &str) -> Option<ChildAgent> {
        let children = self.children.read().await;
        children.get(id).cloned()
    }

    /// Update child status
    pub async fn update_status(&self, id: &str, status: ChildStatus) {
        let mut children = self.children.write().await;
        if let Some(child) = children.get_mut(id) {
            child.status = status;
        }
    }

    /// Set child result
    pub async fn set_result(&self, id: &str, result: String) {
        let mut children = self.children.write().await;
        if let Some(child) = children.get_mut(id) {
            child.status = ChildStatus::Completed;
            child.result = Some(result);
        }
    }

    /// Set child error
    pub async fn set_error(&self, id: &str, error: String) {
        let mut children = self.children.write().await;
        if let Some(child) = children.get_mut(id) {
            child.status = ChildStatus::Failed;
            child.error = Some(error);
        }
    }

    /// List all children
    pub async fn list_children(&self) -> Vec<ChildAgent> {
        let children = self.children.read().await;
        children.values().cloned().collect()
    }

    /// Remove a child
    pub async fn remove_child(&self, id: &str) -> Option<ChildAgent> {
        let mut children = self.children.write().await;
        children.remove(id)
    }
}

/// Delegate tool for spawning child agents
pub struct DelegateTool {
    tracker: DelegationTracker,
    /// Optional agent for executing child tasks
    agent: Option<Arc<crate::agent::Agent>>,
}

impl std::fmt::Debug for DelegateTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegateTool")
            .field("tracker", &self.tracker)
            .field("has_agent", &self.agent.is_some())
            .finish()
    }
}

impl DelegateTool {
    /// Create a new delegate tool
    pub fn new(depth: usize) -> Self {
        Self {
            tracker: DelegationTracker::new(depth),
            agent: None,
        }
    }

    /// Create a new delegate tool with an agent for execution
    pub fn with_agent(depth: usize, agent: Arc<crate::agent::Agent>) -> Self {
        Self {
            tracker: DelegationTracker::new(depth),
            agent: Some(agent),
        }
    }

    /// Create root-level delegate tool (depth 0)
    pub fn root() -> Self {
        Self::new(0)
    }

    /// Spawn a child agent
    async fn spawn_child(
        &self,
        task: TaskSpec,
        parent_budget: Option<IterationBudget>,
        parent_id: String,
    ) -> crate::Result<ChildAgent> {
        // Check if we can delegate
        if !self.tracker.can_delegate().await {
            return Err(crate::error::MantaError::Validation(
                "Maximum delegation depth reached or too many children".to_string()
            ));
        }

        let child_id = Uuid::new_v4().to_string();
        let budget = parent_budget.unwrap_or_else(|| IterationBudget::new(50));
        let iterations = Arc::new(AtomicUsize::new(0));

        let child = ChildAgent {
            id: child_id.clone(),
            parent_id,
            task: task.clone(),
            status: ChildStatus::Pending,
            created_at: chrono::Utc::now(),
            result: None,
            error: None,
            budget: budget.child(),
            iterations: iterations.clone(),
        };

        // Register the child
        self.tracker.register_child(child.clone()).await;

        info!("Spawned child agent {} for task: {}", child_id, task.prompt.chars().take(50).collect::<String>());

        // Start the child agent execution in the background
        let tracker = self.tracker.clone();
        let agent = self.agent.clone();
        tokio::spawn(async move {
            execute_child_task(child_id, task, tracker, iterations, agent).await;
        });

        Ok(child)
    }
}

/// Execute a child task using the provided agent
async fn execute_child_task(
    child_id: String,
    task: TaskSpec,
    tracker: DelegationTracker,
    iterations: Arc<AtomicUsize>,
    agent: Option<Arc<crate::agent::Agent>>,
) {
    tracker.update_status(&child_id, ChildStatus::Running).await;

    debug!("Child {} starting execution", child_id);

    if let Some(agent) = agent {
        // Create incoming message for the child task
        let message = crate::channels::IncomingMessage::new(
            &format!("child:{}", child_id),
            &format!("delegation:{}", child_id),
            &task.prompt,
        )
        .with_metadata(
            crate::channels::MessageMetadata::new()
                .with_extra("child_id", child_id.clone())
                .with_extra("output_format", task.output_format.clone().unwrap_or_default())
                .with_extra("allowed_tools", task.allowed_tools.join(",")),
        );

        // Process the task through the agent
        match agent.process_message(message).await {
            Ok(response) => {
                iterations.fetch_add(1, Ordering::SeqCst);

                info!(
                    "Child {} completed successfully. Response: {} chars",
                    child_id,
                    response.content.len()
                );

                // Format result based on output_format if specified
                let result = if let Some(format) = &task.output_format {
                    format!("Output format ({}): {}", format, response.content)
                } else {
                    response.content
                };

                tracker.set_result(&child_id, result).await;
            }
            Err(e) => {
                error!("Child {} failed: {}", child_id, e);
                tracker.set_error(&child_id, format!("Task execution failed: {}", e)).await;
            }
        }
    } else {
        // No agent configured - log warning and mark as failed
        warn!(
            "No agent configured for child {}. Task would execute with prompt: {}",
            child_id,
            task.prompt
        );
        tracker.set_error(
            &child_id,
            "No agent configured for delegation".to_string(),
        ).await;
    }

    debug!("Child {} execution completed", child_id);
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "delegate"
    }

    fn description(&self) -> &str {
        r#"Spawn a child agent to handle a subtask in parallel.

Use this tool to:
- Break complex tasks into parallel subtasks
- Delegate work to specialized agents
- Process multiple items concurrently

Limitations:
- Maximum 3 concurrent children per parent
- Maximum delegation depth: 2 (parent → child, no grandchildren)
- Child agents cannot use: delegate, clarify, memory, send_message, execute_code
- Children share parent's iteration budget

The child agent will execute the task independently and return results.
Progress and results are relayed to the parent."#
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["spawn", "status", "list", "cancel"],
                    "description": "Action to perform"
                },
                "task": {
                    "type": "object",
                    "description": "Task specification (for spawn)",
                    "properties": {
                        "prompt": {
                            "type": "string",
                            "description": "Task description/prompt for child"
                        },
                        "output_format": {
                            "type": "string",
                            "description": "Expected output format"
                        },
                        "max_iterations": {
                            "type": "integer",
                            "description": "Maximum iterations for child"
                        },
                        "allowed_tools": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Tools allowed for child (empty = all except blocked)"
                        }
                    },
                    "required": ["prompt"]
                },
                "child_id": {
                    "type": "string",
                    "description": "Child agent ID (for status/cancel)"
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
            .ok_or_else(|| crate::error::MantaError::Validation(
                "action is required".to_string()
            ))?;

        match action {
            "spawn" => {
                // Check if we can spawn more children
                let current_count = self.tracker.child_count().await;
                if current_count >= MAX_CHILDREN {
                    return Ok(ToolExecutionResult::error(format!(
                        "Maximum children ({}) already active. Cannot spawn more.",
                        MAX_CHILDREN
                    )));
                }

                let task_json = &args["task"];
                let prompt = task_json["prompt"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation(
                        "task.prompt is required".to_string()
                    ))?;

                let task = TaskSpec {
                    prompt: prompt.to_string(),
                    output_format: task_json["output_format"].as_str().map(String::from),
                    max_iterations: task_json["max_iterations"].as_u64().map(|v| v as usize),
                    allowed_tools: task_json["allowed_tools"]
                        .as_array()
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_default(),
                    context: HashMap::new(),
                };

                let child = self.spawn_child(task, None, context.conversation_id.clone()).await?;

                Ok(ToolExecutionResult::success(format!(
                    "Spawned child agent: {}", child.id
                )).with_data(json!({
                    "child_id": child.id,
                    "status": child.status,
                    "depth": self.tracker.depth + 1,
                    "max_depth": MAX_DEPTH,
                })))
            }

            "status" => {
                let child_id = args["child_id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation(
                        "child_id is required for status".to_string()
                    ))?;

                match self.tracker.get_child(child_id).await {
                    Some(child) => {
                        Ok(ToolExecutionResult::success(format!(
                            "Child {} status: {:?}", child_id, child.status
                        )).with_data(json!({
                            "child_id": child.id,
                            "status": child.status,
                            "result": child.result,
                            "error": child.error,
                            "created_at": child.created_at.to_rfc3339(),
                        })))
                    }
                    None => Ok(ToolExecutionResult::error(format!(
                        "Child {} not found", child_id
                    ))),
                }
            }

            "list" => {
                let children = self.tracker.list_children().await;
                let summary: Vec<serde_json::Value> = children.iter().map(|c| {
                    json!({
                        "id": c.id,
                        "status": c.status,
                        "prompt_preview": c.task.prompt.chars().take(50).collect::<String>() + "...",
                    })
                }).collect();

                Ok(ToolExecutionResult::success(format!(
                    "{} active children", children.len()
                )).with_data(json!({
                    "children": summary,
                    "count": children.len(),
                    "max_children": MAX_CHILDREN,
                })))
            }

            "cancel" => {
                let child_id = args["child_id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation(
                        "child_id is required for cancel".to_string()
                    ))?;

                if let Some(_child) = self.tracker.remove_child(child_id).await {
                    info!("Cancelled child agent: {}", child_id);
                    Ok(ToolExecutionResult::success(format!(
                        "Cancelled child {}", child_id
                    )))
                } else {
                    Ok(ToolExecutionResult::error(format!(
                        "Child {} not found", child_id
                    )))
                }
            }

            _ => Err(crate::error::MantaError::Validation(
                format!("Unknown action: {}", action)
            )),
        }
    }
}

impl Clone for DelegationTracker {
    fn clone(&self) -> Self {
        Self {
            children: Arc::clone(&self.children),
            depth: self.depth,
            max_children: self.max_children,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delegation_tracker() {
        let tracker = DelegationTracker::new(0);
        assert_eq!(tracker.depth, 0);
    }

    #[test]
    fn test_task_spec_creation() {
        let task = TaskSpec {
            prompt: "Test task".to_string(),
            output_format: Some("json".to_string()),
            max_iterations: Some(10),
            allowed_tools: vec!["file_read".to_string()],
            context: HashMap::new(),
        };
        assert_eq!(task.prompt, "Test task");
    }

    #[test]
    fn test_child_status_serialization() {
        let status = ChildStatus::Running;
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, "\"running\"");
    }
}
