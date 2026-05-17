//! Cron Tool for Manta
//!
//! This tool allows the AI to schedule recurring tasks using cron expressions.
//! Jobs are delegated to the CronScheduler for execution.
//!
//! Note: This tool has been refactored to use CronScheduler as the
//! single source of truth for all cron functionality.

use super::{create_schema, Tool, ToolContext, ToolExecutionResult};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::cron::cron::{CronJob, CronScheduler, DeliveryMode, ExecutionTarget, Schedule};
use std::str::FromStr;

/// Global scheduler reference for CronTool
/// This is set by the Gateway after the CronScheduler is initialized
static SCHEDULER: tokio::sync::OnceCell<Arc<Mutex<CronScheduler>>> =
    tokio::sync::OnceCell::const_new();

/// Cron tool for scheduling recurring tasks
#[derive(Debug)]
pub struct CronTool;

impl CronTool {
    /// Create a new cron tool
    /// Note: The tool delegates all operations to CronScheduler.
    /// Call `CronTool::set_scheduler()` after CronScheduler is initialized.
    pub fn new() -> Self {
        Self
    }

    /// Set the global scheduler reference
    /// This should be called once during Gateway initialization
    pub fn set_scheduler(scheduler: Arc<Mutex<CronScheduler>>) {
        if SCHEDULER.set(scheduler).is_err() {
            warn!("CronTool scheduler already set, ignoring duplicate");
        }
    }

    /// Get the scheduler if initialized
    fn scheduler() -> Option<Arc<Mutex<CronScheduler>>> {
        SCHEDULER.get().cloned()
    }

    /// Check if scheduler is available
    pub fn is_ready() -> bool {
        SCHEDULER.get().is_some()
    }
}

impl Default for CronTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Schedule and manage recurring tasks using cron expressions. \
         Can create, list, enable, disable, and remove scheduled jobs. \
         Jobs run automatically in the background according to their schedule. \
         Cron format: 'minute hour day month weekday' (e.g., '0 * * * *' = hourly, '*/5 * * * *' = every 5 minutes)"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        create_schema(
            "Cron scheduler tool",
            json!({
                "action": {
                    "type": "string",
                    "enum": ["create", "list", "enable", "disable", "remove", "run"],
                    "description": "Action to perform"
                },
                "name": {
                    "type": "string",
                    "description": "Job name (required for create, enable, disable, remove, run)"
                },
                "schedule": {
                    "type": "string",
                    "description": "Cron schedule expression (required for create). Examples: '*/5 * * * *' = every 5 min, '0 * * * *' = hourly, '0 2 * * *' = daily at 2am"
                },
                "command": {
                    "type": "string",
                    "description": "Command to execute (required for create)"
                },
                "description": {
                    "type": "string",
                    "description": "Optional job description"
                }
            }),
            vec!["action"],
        )
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        // Check if scheduler is available
        let scheduler = match Self::scheduler() {
            Some(s) => s,
            None => {
                return Ok(ToolExecutionResult::error(
                    "Cron scheduler is not yet initialized. Please try again in a moment.",
                ));
            }
        };

        let action = args.get("action").and_then(|v| v.as_str()).ok_or_else(|| {
            crate::error::MantaError::Validation("Missing 'action' parameter".to_string())
        })?;

        match action {
            "create" => {
                let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::MantaError::Validation("Missing 'name' parameter".to_string())
                })?;
                let schedule_str =
                    args.get("schedule")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            crate::error::MantaError::Validation(
                                "Missing 'schedule' parameter".to_string(),
                            )
                        })?;
                let command = args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        crate::error::MantaError::Validation(
                            "Missing 'command' parameter".to_string(),
                        )
                    })?;

                // Parse cron schedule using the cron crate
                // The cron crate v0.14 expects 6 fields (with seconds), so we need to
                // convert 5-field expressions to 6-field by prepending "0" for seconds
                let normalized_schedule = if schedule_str.trim().split_whitespace().count() == 5 {
                    format!("0 {}", schedule_str.trim())
                } else {
                    schedule_str.to_string()
                };

                let schedule = match cron::Schedule::from_str(&normalized_schedule) {
                    Ok(_) => Schedule::Cron {
                        expression: schedule_str.to_string(), // Store original 5-field format
                        timezone: None,
                        stagger_ms: None,
                    },
                    Err(e) => {
                        return Ok(ToolExecutionResult::error(&format!(
                            "Invalid cron expression '{}': {}",
                            schedule_str, e
                        )));
                    }
                };

                let job_id = uuid::Uuid::new_v4().to_string();

                // Create the job with shell execution target and announce delivery
                let job = CronJob::new(
                    job_id.clone(),
                    name,
                    schedule,
                    ExecutionTarget::Shell { command: command.to_string() },
                )
                .with_delivery(DeliveryMode::Announce {
                    channel: "web_terminal".to_string(),
                    to: "*".to_string(),
                });

                // Add job to scheduler
                let guard = scheduler.lock().await;
                guard.add_job(job).await?;
                drop(guard);

                info!("Created cron job '{}' with schedule '{}'", name, schedule_str);

                Ok(ToolExecutionResult::success(
                    &format!("✅ Created cron job '{}'\nSchedule: {}\nCommand: {}\n\nThe job is now active and will run automatically according to the schedule.", name, schedule_str, command)
                ))
            }
            "list" => {
                let guard = scheduler.lock().await;
                let jobs = guard.list_jobs().await;
                drop(guard);

                if jobs.is_empty() {
                    return Ok(ToolExecutionResult::success(
                        "No cron jobs configured. Use 'create' action to add a job.",
                    ));
                }

                let mut output = format!("📅 Cron Jobs ({} total)\n", jobs.len());
                output.push_str(&"=".repeat(50));
                output.push('\n');

                for job in jobs.iter() {
                    let status = if job.enabled { "✅" } else { "❌" };
                    output.push_str(&format!("\n{} {}\n", status, job.name));
                    output.push_str(&format!("   ID: {}\n", job.id));
                    match &job.schedule {
                        Schedule::Cron { expression, .. } => {
                            output.push_str(&format!("   Schedule: {}\n", expression));
                        }
                        Schedule::At { timestamp } => {
                            output.push_str(&format!("   Schedule: once at {}\n", timestamp));
                        }
                        Schedule::Every { interval, .. } => {
                            output.push_str(&format!("   Schedule: every {:?}\n", interval));
                        }
                    }
                    match &job.target {
                        ExecutionTarget::Shell { command } => {
                            output.push_str(&format!("   Command: {}\n", command));
                        }
                        ExecutionTarget::Agent { prompt, agent_id, .. } => {
                            output.push_str(&format!(
                                "   Agent: {}\n",
                                agent_id.as_deref().unwrap_or("default")
                            ));
                            output.push_str(&format!(
                                "   Prompt: {}...\n",
                                &prompt[..prompt.len().min(50)]
                            ));
                        }
                    }
                    output.push_str(&format!("   Run count: {}\n", job.state.run_count));
                    if let Some(last) = job.state.last_run_at {
                        output.push_str(&format!("   Last run: {}\n", last.to_rfc3339()));
                    }
                    if let Some(next) = job.state.next_run_at {
                        output.push_str(&format!("   Next run: {}\n", next.to_rfc3339()));
                    }
                }

                Ok(ToolExecutionResult::success(&output))
            }
            "enable" => {
                let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::MantaError::Validation("Missing 'name' parameter".to_string())
                })?;

                let guard = scheduler.lock().await;
                let jobs = guard.list_jobs().await;

                // Find job by name
                if let Some(job) = jobs.iter().find(|j| j.name == name) {
                    guard.set_job_enabled(&job.id, true).await?;
                    drop(guard);
                    Ok(ToolExecutionResult::success(&format!("✅ Enabled cron job '{}'", name)))
                } else {
                    drop(guard);
                    Ok(ToolExecutionResult::error(&format!("Job '{}' not found", name)))
                }
            }
            "disable" => {
                let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::MantaError::Validation("Missing 'name' parameter".to_string())
                })?;

                let guard = scheduler.lock().await;
                let jobs = guard.list_jobs().await;

                // Find job by name
                if let Some(job) = jobs.iter().find(|j| j.name == name) {
                    guard.set_job_enabled(&job.id, false).await?;
                    drop(guard);
                    Ok(ToolExecutionResult::success(&format!("✅ Disabled cron job '{}'", name)))
                } else {
                    drop(guard);
                    Ok(ToolExecutionResult::error(&format!("Job '{}' not found", name)))
                }
            }
            "remove" | "delete" => {
                let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::MantaError::Validation("Missing 'name' parameter".to_string())
                })?;

                let guard = scheduler.lock().await;
                let jobs = guard.list_jobs().await;

                // Find job by name
                if let Some(job) = jobs.iter().find(|j| j.name == name) {
                    guard.remove_job(&job.id).await?;
                    drop(guard);
                    Ok(ToolExecutionResult::success(&format!("✅ Removed cron job '{}'", name)))
                } else {
                    drop(guard);
                    Ok(ToolExecutionResult::error(&format!("Job '{}' not found", name)))
                }
            }
            "run" => {
                let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
                    crate::error::MantaError::Validation("Missing 'name' parameter".to_string())
                })?;

                let guard = scheduler.lock().await;
                let jobs = guard.list_jobs().await;

                // Find job by name
                if let Some(job) = jobs.iter().find(|j| j.name == name) {
                    guard.trigger_job(&job.id).await?;
                    drop(guard);
                    Ok(ToolExecutionResult::success(&format!("🔄 Triggered cron job '{}'", name)))
                } else {
                    drop(guard);
                    Ok(ToolExecutionResult::error(&format!("Job '{}' not found", name)))
                }
            }
            _ => Ok(ToolExecutionResult::error(&format!(
                "Unknown action: {}. Use: create, list, enable, disable, remove, run",
                action
            ))),
        }
    }

    /// Check if this tool is available in the given context
    fn is_available(&self, _context: &ToolContext) -> bool {
        // Cron tool is available if the scheduler is initialized
        Self::is_ready()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cron_tool_new() {
        let tool = CronTool::new();
        assert_eq!(tool.name(), "cron");
    }

    #[test]
    fn test_cron_tool_default() {
        let tool = CronTool::default();
        assert_eq!(tool.name(), "cron");
    }

    #[test]
    fn test_cron_tool_is_ready_false_by_default() {
        assert!(!CronTool::is_ready());
    }

    #[tokio::test]
    async fn test_cron_tool_description() {
        let tool = CronTool::new();
        assert!(tool.description().contains("cron"));
    }

    #[tokio::test]
    async fn test_cron_tool_execute_without_scheduler() {
        let tool = CronTool::new();
        let result = tool
            .execute(serde_json::json!({"action": "list"}), &ToolContext::default())
            .await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(!r.success);
        assert!(r.error.as_ref().unwrap().contains("not yet initialized"));
    }

    #[tokio::test]
    async fn test_cron_tool_no_scheduler_returns_error() {
        let tool = CronTool::new();
        // Without scheduler set, any execution returns an Ok-wrapped error
        let result = tool
            .execute(serde_json::json!({"action": "list"}), &ToolContext::default())
            .await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(!r.success);
        assert!(r.error.as_ref().unwrap().contains("not yet initialized"));
    }
}
