//! Heartbeat Tool - Agent self-management of periodic tasks
//!
//! Allows agents to read and update their own HEARTBEAT.md to manage
//! periodic tasks that the heartbeat runner will execute.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::json;
use tracing::{error, info};

use super::{Tool, ToolContext, ToolExecutionResult};
use crate::heartbeat::parser::{parse_heartbeat_tasks, HeartbeatTask};

/// Tool for managing agent heartbeat tasks
#[derive(Debug)]
pub struct HeartbeatTool;

impl HeartbeatTool {
    /// Create a new heartbeat tool
    pub fn new() -> Self {
        Self
    }

    /// Get the path to an agent's HEARTBEAT.md
    fn heartbeat_path(&self, agent_id: &str) -> PathBuf {
        crate::dirs::agents_dir()
            .join(agent_id)
            .join("HEARTBEAT.md")
    }

    /// Read and parse HEARTBEAT.md for an agent
    async fn read_heartbeat(&self, agent_id: &str) -> crate::Result<(String, Vec<HeartbeatTask>)> {
        let path = self.heartbeat_path(agent_id);
        let content = if path.exists() {
            match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to read HEARTBEAT.md for {}: {}", agent_id, e);
                    return Err(crate::error::SyscityError::ExternalService {
                        source: format!("Failed to read HEARTBEAT.md: {}", e),
                        cause: Some(Box::new(e)),
                    });
                }
            }
        } else {
            String::new()
        };

        let tasks = parse_heartbeat_tasks(&content);
        Ok((content, tasks))
    }

    /// Write HEARTBEAT.md for an agent
    async fn write_heartbeat(&self, agent_id: &str, content: &str) -> crate::Result<()> {
        let path = self.heartbeat_path(agent_id);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                error!("Failed to create agent directory for {}: {}", agent_id, e);
                return Err(crate::error::SyscityError::ExternalService {
                    source: format!("Failed to create directory: {}", e),
                    cause: Some(Box::new(e)),
                });
            }
        }

        match tokio::fs::write(&path, content).await {
            Ok(_) => {
                info!("Updated HEARTBEAT.md for agent {}", agent_id);
                Ok(())
            }
            Err(e) => {
                error!("Failed to write HEARTBEAT.md for {}: {}", agent_id, e);
                Err(crate::error::SyscityError::ExternalService {
                    source: format!("Failed to write HEARTBEAT.md: {}", e),
                    cause: Some(Box::new(e)),
                })
            }
        }
    }

    /// Format tasks into HEARTBEAT.md content
    fn format_heartbeat(tasks: &[HeartbeatTask]) -> String {
        let mut output = String::from("# Heartbeat Tasks\n\n");
        if tasks.is_empty() {
            output.push_str("<!-- No tasks configured -->\n");
            return output;
        }
        output.push_str("tasks:\n");
        for task in tasks {
            output.push_str(&format!(
                "  - name: {}\n    interval: {}\n    prompt: \"{}\"\n",
                task.name,
                Self::format_duration(task.interval),
                task.prompt.replace('"', "\\\"")
            ));
        }
        output
    }

    /// Format a Duration back to human-readable string
    fn format_duration(d: std::time::Duration) -> String {
        let secs = d.as_secs();
        if secs >= 3600 && secs.is_multiple_of(3600) {
            format!("{}h", secs / 3600)
        } else if secs >= 60 && secs.is_multiple_of(60) {
            format!("{}m", secs / 60)
        } else {
            format!("{}s", secs)
        }
    }
}

impl Default for HeartbeatTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HeartbeatTool {
    fn name(&self) -> &str {
        "heartbeat"
    }

    fn description(&self) -> &str {
        r#"Manage your own periodic heartbeat tasks in HEARTBEAT.md.

USE THIS TOOL when the user asks to add, remove, view, or modify your "heartbeat tasks" (心跳任务) or HEARTBEAT.md. These are tasks that YOU (the agent) periodically check and execute yourself via the heartbeat scheduler.

DO NOT use this tool for general cron jobs, shell scripts, or external system tasks — use the cron tool for those.

Actions:
- read: List all current heartbeat tasks
- update: Replace entire HEARTBEAT.md content
- add_task: Add a single task to the existing list
- remove_task: Remove a task by name

Your agent_id is provided in your system prompt under "Agent Identity".
You may also use "me" or "self" as agent_id to refer to yourself."#
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "update", "add_task", "remove_task"],
                    "description": "The action to perform"
                },
                "agent_id": {
                    "type": "string",
                    "description": "Your agent ID (from system prompt) or 'me'/'self' to refer to yourself"
                },
                "content": {
                    "type": "string",
                    "description": "Full HEARTBEAT.md content (required for update action)"
                },
                "task_name": {
                    "type": "string",
                    "description": "Task name (required for add_task and remove_task)"
                },
                "task_interval": {
                    "type": "string",
                    "description": "Task interval like '5m', '1h', '30s' (required for add_task)"
                },
                "task_prompt": {
                    "type": "string",
                    "description": "The prompt/instruction for this task (required for add_task)"
                }
            },
            "required": ["action", "agent_id"]
        })
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args["action"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("action is required".to_string())
        })?;

        let raw_agent_id = args["agent_id"].as_str().ok_or_else(|| {
            crate::error::SyscityError::Validation("agent_id is required".to_string())
        })?;

        // Resolve "me" or "self" to the actual agent ID
        let agent_id = if raw_agent_id == "me" || raw_agent_id == "self" {
            // The agent should know its own ID from the system prompt.
            // If it passes "me"/"self", we can't resolve it without context.
            // Require explicit agent_id for now.
            return Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(
                    "Please provide your actual agent_id (from your system prompt under 'Agent \
                     Identity') instead of 'me' or 'self'."
                        .to_string(),
                ),
                data: None,
                execution_time: std::time::Duration::default(),
            });
        } else {
            raw_agent_id
        };

        // Validate agent_id is a reasonable directory name (basic sanitization)
        if agent_id.contains('/') || agent_id.contains('\\') || agent_id.contains("..") {
            return Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some("Invalid agent_id: contains path separators".to_string()),
                data: None,
                execution_time: std::time::Duration::default(),
            });
        }

        match action {
            "read" => {
                let (content, tasks) = self.read_heartbeat(agent_id).await?;

                let mut output =
                    format!("HEARTBEAT.md for agent '{}' ({} tasks):\n\n", agent_id, tasks.len());

                if tasks.is_empty() {
                    output.push_str("No tasks configured.\n");
                } else {
                    for (i, task) in tasks.iter().enumerate() {
                        output.push_str(&format!(
                            "{}. {} (every {})\n   Prompt: {}\n\n",
                            i + 1,
                            task.name,
                            Self::format_duration(task.interval),
                            task.prompt
                        ));
                    }
                }

                let mut result = ToolExecutionResult::success(output);
                result.data = Some(json!({
                    "agent_id": agent_id,
                    "content": content,
                    "tasks": tasks.iter().map(|t| json!({
                        "name": t.name,
                        "interval": Self::format_duration(t.interval),
                        "prompt": t.prompt,
                    })).collect::<Vec<_>>(),
                    "count": tasks.len(),
                }));
                Ok(result)
            }

            "update" => {
                let content = args["content"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "content is required for update action".to_string(),
                    )
                })?;

                self.write_heartbeat(agent_id, content).await?;

                let tasks = parse_heartbeat_tasks(content);
                let mut result = ToolExecutionResult::success(format!(
                    "Updated HEARTBEAT.md for agent '{}'. {} tasks configured.",
                    agent_id,
                    tasks.len()
                ));
                result.data = Some(json!({
                    "agent_id": agent_id,
                    "count": tasks.len(),
                }));
                Ok(result)
            }

            "add_task" => {
                let task_name = args["task_name"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "task_name is required for add_task action".to_string(),
                    )
                })?;

                let task_interval = args["task_interval"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "task_interval is required for add_task action".to_string(),
                    )
                })?;

                let task_prompt = args["task_prompt"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "task_prompt is required for add_task action".to_string(),
                    )
                })?;

                let (_content, mut tasks) = self.read_heartbeat(agent_id).await?;

                // Check for duplicate name
                if tasks.iter().any(|t| t.name == task_name) {
                    return Ok(ToolExecutionResult {
                        success: false,
                        output: format!(
                            "Task '{}' already exists. Use remove_task first, or update to \
                             replace all tasks.",
                            task_name
                        ),
                        error: Some(format!("Duplicate task name: {}", task_name)),
                        data: None,
                        execution_time: std::time::Duration::default(),
                    });
                }

                // Parse interval
                let interval =
                    crate::heartbeat::parser::parse_duration(task_interval).ok_or_else(|| {
                        crate::error::SyscityError::Validation(format!(
                            "Invalid interval format: {}. Use formats like '5m', '1h', '30s'.",
                            task_interval
                        ))
                    })?;

                tasks.push(HeartbeatTask {
                    name: task_name.to_string(),
                    interval,
                    prompt: task_prompt.to_string(),
                });

                let new_content = Self::format_heartbeat(&tasks);
                self.write_heartbeat(agent_id, &new_content).await?;

                let mut result = ToolExecutionResult::success(format!(
                    "Added task '{}' to agent '{}' heartbeat. {} total tasks.",
                    task_name,
                    agent_id,
                    tasks.len()
                ));
                result.data = Some(json!({
                    "agent_id": agent_id,
                    "added": task_name,
                    "total": tasks.len(),
                }));
                Ok(result)
            }

            "remove_task" => {
                let task_name = args["task_name"].as_str().ok_or_else(|| {
                    crate::error::SyscityError::Validation(
                        "task_name is required for remove_task action".to_string(),
                    )
                })?;

                let (_content, mut tasks) = self.read_heartbeat(agent_id).await?;

                let before_len = tasks.len();
                tasks.retain(|t| t.name != task_name);

                if tasks.len() == before_len {
                    return Ok(ToolExecutionResult {
                        success: false,
                        output: format!(
                            "Task '{}' not found in agent '{}' heartbeat.",
                            task_name, agent_id
                        ),
                        error: Some(format!("Task not found: {}", task_name)),
                        data: None,
                        execution_time: std::time::Duration::default(),
                    });
                }

                let new_content = Self::format_heartbeat(&tasks);
                self.write_heartbeat(agent_id, &new_content).await?;

                let mut result = ToolExecutionResult::success(format!(
                    "Removed task '{}' from agent '{}' heartbeat. {} tasks remaining.",
                    task_name,
                    agent_id,
                    tasks.len()
                ));
                result.data = Some(json!({
                    "agent_id": agent_id,
                    "removed": task_name,
                    "remaining": tasks.len(),
                }));
                Ok(result)
            }

            _ => Ok(ToolExecutionResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Unknown action: {}. Use 'read', 'update', 'add_task', or 'remove_task'.",
                    action
                )),
                data: None,
                execution_time: std::time::Duration::default(),
            }),
        }
    }
}
