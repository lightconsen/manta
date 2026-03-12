//! Cron Tool for Manta
//!
//! This tool allows the AI to schedule recurring tasks using cron expressions.

use super::{Tool, ToolContext, ToolExecutionResult, create_schema};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info, warn};

/// Cron tool for scheduling recurring tasks
#[derive(Debug)]
pub struct CronTool;

impl CronTool {
    /// Create a new cron tool
    pub fn new() -> Self {
        Self
    }

    /// Get the jobs file path
    fn jobs_file() -> std::path::PathBuf {
        dirs::data_dir()
            .map(|p| p.join("manta").join("cron_jobs.json"))
            .unwrap_or_else(|| std::path::PathBuf::from(".manta_cron_jobs.json"))
    }

    /// Load existing jobs
    async fn load_jobs(&self) -> Vec<CronJobEntry> {
        let jobs_file = Self::jobs_file();
        if !jobs_file.exists() {
            return Vec::new();
        }

        match tokio::fs::read_to_string(&jobs_file).await {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(e) => {
                warn!("Failed to read jobs file: {}", e);
                Vec::new()
            }
        }
    }

    /// Save jobs to file
    async fn save_jobs(&self, jobs: &[CronJobEntry]) -> crate::Result<()> {
        let jobs_file = Self::jobs_file();
        if let Some(parent) = jobs_file.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let content = serde_json::to_string_pretty(jobs)?;
        tokio::fs::write(&jobs_file, content).await
            .map_err(|e| crate::error::MantaError::Io(e))?;
        Ok(())
    }
}

impl Default for CronTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Cron job entry
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CronJobEntry {
    id: String,
    name: String,
    schedule: String,
    command: String,
    description: String,
    enabled: bool,
    created_at: String,
    run_count: u32,
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Schedule and manage recurring tasks using cron expressions. \
         Can create, list, enable, disable, and remove scheduled jobs. \
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
            vec!["action"]
        )
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args.get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'action' parameter".to_string()))?;

        match action {
            "create" => {
                let name = args.get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'name' parameter".to_string()))?;
                let schedule = args.get("schedule")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'schedule' parameter".to_string()))?;
                let command = args.get("command")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'command' parameter".to_string()))?;
                let description = args.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Validate cron schedule (basic check)
                let parts: Vec<&str> = schedule.split_whitespace().collect();
                if parts.len() != 5 {
                    return Ok(ToolExecutionResult::error(
                        "Invalid schedule format. Use: 'minute hour day month weekday' (5 parts)"
                    ));
                }

                let mut jobs = self.load_jobs().await;

                // Check for duplicate name
                if jobs.iter().any(|j| j.name == name) {
                    return Ok(ToolExecutionResult::error(
                        &format!("Job '{}' already exists. Use a different name or remove the existing job first.", name)
                    ));
                }

                let job = CronJobEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: name.to_string(),
                    schedule: schedule.to_string(),
                    command: command.to_string(),
                    description: description.to_string(),
                    enabled: true,
                    created_at: chrono::Utc::now().to_rfc3339(),
                    run_count: 0,
                };

                jobs.push(job);
                self.save_jobs(&jobs).await?;

                info!("Created cron job '{}' with schedule '{}'", name, schedule);

                Ok(ToolExecutionResult::success(
                    &format!("✅ Created cron job '{}'\nSchedule: {}\nCommand: {}\n\nThe job is now active and will run according to the schedule.", name, schedule, command)
                ))
            }
            "list" => {
                let jobs = self.load_jobs().await;

                if jobs.is_empty() {
                    return Ok(ToolExecutionResult::success("No cron jobs configured. Use 'create' action to add a job."));
                }

                let mut output = format!("📅 Cron Jobs ({} total)\n", jobs.len());
                output.push_str("=" .repeat(50).as_str());
                output.push('\n');

                for job in &jobs {
                    let status = if job.enabled { "✅" } else { "❌" };
                    output.push_str(&format!("\n{} {}\n", status, job.name));
                    output.push_str(&format!("   Schedule: {}\n", job.schedule));
                    output.push_str(&format!("   Command: {}\n", job.command));
                    if !job.description.is_empty() {
                        output.push_str(&format!("   Description: {}\n", job.description));
                    }
                    output.push_str(&format!("   Run count: {}\n", job.run_count));
                }

                Ok(ToolExecutionResult::success(&output))
            }
            "enable" => {
                let name = args.get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'name' parameter".to_string()))?;

                let mut jobs = self.load_jobs().await;

                if let Some(job) = jobs.iter_mut().find(|j| j.name == name) {
                    job.enabled = true;
                    self.save_jobs(&jobs).await?;
                    Ok(ToolExecutionResult::success(&format!("✅ Enabled cron job '{}'", name)))
                } else {
                    Ok(ToolExecutionResult::error(&format!("Job '{}' not found", name)))
                }
            }
            "disable" => {
                let name = args.get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'name' parameter".to_string()))?;

                let mut jobs = self.load_jobs().await;

                if let Some(job) = jobs.iter_mut().find(|j| j.name == name) {
                    job.enabled = false;
                    self.save_jobs(&jobs).await?;
                    Ok(ToolExecutionResult::success(&format!("✅ Disabled cron job '{}'", name)))
                } else {
                    Ok(ToolExecutionResult::error(&format!("Job '{}' not found", name)))
                }
            }
            "remove" | "delete" => {
                let name = args.get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'name' parameter".to_string()))?;

                let mut jobs = self.load_jobs().await;
                let initial_len = jobs.len();
                jobs.retain(|j| j.name != name);

                if jobs.len() == initial_len {
                    return Ok(ToolExecutionResult::error(&format!("Job '{}' not found", name)));
                }

                self.save_jobs(&jobs).await?;
                Ok(ToolExecutionResult::success(&format!("✅ Removed cron job '{}'", name)))
            }
            "run" => {
                let name = args.get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'name' parameter".to_string()))?;

                let jobs = self.load_jobs().await;

                if let Some(job) = jobs.iter().find(|j| j.name == name) {
                    let mut output = format!("🔄 Running cron job '{}'\n", name);
                    output.push_str(&format!("Command: {}\n", job.command));
                    output.push_str("\nNote: This is a manual trigger. The job will also run automatically according to its schedule.\n");
                    output.push_str("To execute the command now, use the shell tool with the command above.");

                    Ok(ToolExecutionResult::success(&output))
                } else {
                    Ok(ToolExecutionResult::error(&format!("Job '{}' not found", name)))
                }
            }
            _ => {
                Ok(ToolExecutionResult::error(&format!("Unknown action: {}. Use: create, list, enable, disable, remove, run", action)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cron_tool_new() {
        let tool = CronTool::new();
        assert_eq!(tool.name(), "cron");
    }
}
