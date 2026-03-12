//! Cron Tool for Manta
//!
//! This tool allows the AI to schedule recurring tasks using cron expressions.
//! Jobs are stored in a JSON file and executed by a background scheduler.

use super::{Tool, ToolContext, ToolExecutionResult, create_schema};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};
use chrono::{Utc, Datelike, Timelike};

/// Cron tool for scheduling recurring tasks
#[derive(Debug)]
pub struct CronTool {
    jobs: Arc<RwLock<Vec<CronJobEntry>>>,
}

impl CronTool {
    /// Create a new cron tool and start the scheduler
    pub fn new() -> Self {
        let jobs = Arc::new(RwLock::new(Vec::new()));
        let tool = Self { jobs: Arc::clone(&jobs) };

        // Load existing jobs and defer scheduler startup
        let jobs_clone = Arc::clone(&jobs);
        tokio::spawn(async move {
            // Load jobs first
            if let Ok(loaded) = load_jobs_from_file().await {
                let mut guard = jobs_clone.write().await;
                *guard = loaded;
                info!("Loaded {} cron jobs", guard.len());
                drop(guard);
            }

            // Defer scheduler start by 2 seconds to let UI initialize
            tokio::time::sleep(Duration::from_secs(2)).await;

            // Start the background scheduler
            scheduler_loop(Arc::clone(&jobs_clone)).await;
        });

        tool
    }

    /// Get the jobs file path
    fn jobs_file() -> std::path::PathBuf {
        dirs::data_dir()
            .map(|p| p.join("manta").join("cron_jobs.json"))
            .unwrap_or_else(|| std::path::PathBuf::from(".manta_cron_jobs.json"))
    }

    /// Save jobs to file
    async fn save_jobs(&self, jobs: &[CronJobEntry]) -> crate::Result<()> {
        save_jobs_to_file(jobs).await
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
    last_run: Option<String>,
}

/// Load jobs from file
async fn load_jobs_from_file() -> crate::Result<Vec<CronJobEntry>> {
    let jobs_file = CronTool::jobs_file();
    if !jobs_file.exists() {
        return Ok(Vec::new());
    }

    match tokio::fs::read_to_string(&jobs_file).await {
        Ok(content) => {
            let jobs: Vec<CronJobEntry> = serde_json::from_str(&content).unwrap_or_default();
            Ok(jobs)
        }
        Err(e) => {
            warn!("Failed to read jobs file: {}", e);
            Ok(Vec::new())
        }
    }
}

/// Save jobs to file
async fn save_jobs_to_file(jobs: &[CronJobEntry]) -> crate::Result<()> {
    let jobs_file = CronTool::jobs_file();
    if let Some(parent) = jobs_file.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let content = serde_json::to_string_pretty(jobs)?;
    tokio::fs::write(&jobs_file, content).await
        .map_err(|e| crate::error::MantaError::Io(e))?;
    Ok(())
}

/// Background scheduler loop
async fn scheduler_loop(jobs: Arc<RwLock<Vec<CronJobEntry>>>) {
    let mut tick = interval(Duration::from_secs(30)); // Check every 30 seconds

    loop {
        tick.tick().await;

        let now = Utc::now();
        let current_minute = now.minute();
        let current_hour = now.hour();
        let current_day = now.day();
        let current_month = now.month();
        let current_weekday = now.weekday().num_days_from_sunday();

        let mut jobs_to_run: Vec<(String, String)> = Vec::new();

        // Check which jobs are due
        {
            let jobs_guard = jobs.read().await;
            for job in jobs_guard.iter() {
                if !job.enabled {
                    continue;
                }

                if is_due(&job.schedule, current_minute, current_hour, current_day, current_month, current_weekday) {
                    jobs_to_run.push((job.id.clone(), job.command.clone()));
                }
            }
        }

        // Check if any jobs need to run
        let jobs_count = jobs_to_run.len();

        // Execute due jobs
        for (job_id, command) in jobs_to_run {
            debug!("Executing cron job '{}' with command: {}", job_id, command);

            // Execute the command
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .output()
                .await;

            match output {
                Ok(result) => {
                    let stdout = String::from_utf8_lossy(&result.stdout);
                    let stderr = String::from_utf8_lossy(&result.stderr);

                    if result.status.success() {
                        // Print output cleanly to stdout so user sees it
                        if !stdout.trim().is_empty() {
                            println!("{}", stdout.trim());
                        }
                        debug!("Job '{}' executed successfully", job_id);
                    } else {
                        eprintln!("Job failed: {}", stderr.trim());
                        error!("Job '{}' failed. Stderr: {}", job_id, stderr.trim());
                    }
                }
                Err(e) => {
                    error!("Failed to execute job '{}': {}", job_id, e);
                }
            }

            // Update job run count and last_run
            let mut jobs_guard = jobs.write().await;
            if let Some(job) = jobs_guard.iter_mut().find(|j| j.id == job_id) {
                job.run_count += 1;
                job.last_run = Some(Utc::now().to_rfc3339());
            }

            // Save updated jobs
            let jobs_vec = jobs_guard.clone();
            drop(jobs_guard);

            if let Err(e) = save_jobs_to_file(&jobs_vec).await {
                error!("Failed to save jobs after execution: {}", e);
            }
        }

        // Re-print prompt after jobs execute so user knows where to type
        if jobs_count > 0 {
            println!("\n💬 You > ");
        }
    }
}

/// Check if a job is due based on its cron schedule
fn is_due(schedule: &str, minute: u32, hour: u32, day: u32, month: u32, weekday: u32) -> bool {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return false;
    }

    // Parse each field
    let minute_match = matches_field(parts[0], minute, 0, 59);
    let hour_match = matches_field(parts[1], hour, 0, 23);
    let day_match = matches_field(parts[2], day, 1, 31);
    let month_match = matches_field(parts[3], month, 1, 12);
    let weekday_match = matches_field(parts[4], weekday, 0, 6);

    minute_match && hour_match && day_match && month_match && weekday_match
}

/// Check if a value matches a cron field pattern
fn matches_field(pattern: &str, value: u32, min: u32, max: u32) -> bool {
    if pattern == "*" {
        return true;
    }

    // Handle step values like */5
    if pattern.starts_with("*/") {
        if let Ok(step) = pattern[2..].parse::<u32>() {
            return value % step == 0;
        }
    }

    // Handle ranges like 1-5
    if pattern.contains('-') {
        let range: Vec<&str> = pattern.split('-').collect();
        if range.len() == 2 {
            if let (Ok(start), Ok(end)) = (range[0].parse::<u32>(), range[1].parse::<u32>()) {
                return value >= start && value <= end;
            }
        }
    }

    // Handle lists like 1,2,3
    if pattern.contains(',') {
        for part in pattern.split(',') {
            if let Ok(v) = part.parse::<u32>() {
                if v == value {
                    return true;
                }
            }
        }
        return false;
    }

    // Handle single value
    if let Ok(v) = pattern.parse::<u32>() {
        return v == value;
    }

    true // Default to true for unknown patterns
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

                let mut jobs = self.jobs.write().await;

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
                    last_run: None,
                };

                jobs.push(job);
                self.save_jobs(&jobs).await?;
                drop(jobs);

                info!("Created cron job '{}' with schedule '{}'", name, schedule);

                Ok(ToolExecutionResult::success(
                    &format!("✅ Created cron job '{}'\nSchedule: {}\nCommand: {}\n\nThe job is now active and will run automatically according to the schedule.", name, schedule, command)
                ))
            }
            "list" => {
                let jobs = self.jobs.read().await;

                if jobs.is_empty() {
                    return Ok(ToolExecutionResult::success("No cron jobs configured. Use 'create' action to add a job."));
                }

                let mut output = format!("📅 Cron Jobs ({} total)\n", jobs.len());
                output.push_str(&"=".repeat(50));
                output.push('\n');

                for job in jobs.iter() {
                    let status = if job.enabled { "✅" } else { "❌" };
                    output.push_str(&format!("\n{} {}\n", status, job.name));
                    output.push_str(&format!("   Schedule: {}\n", job.schedule));
                    output.push_str(&format!("   Command: {}\n", job.command));
                    if !job.description.is_empty() {
                        output.push_str(&format!("   Description: {}\n", job.description));
                    }
                    output.push_str(&format!("   Run count: {}\n", job.run_count));
                    if let Some(last) = &job.last_run {
                        output.push_str(&format!("   Last run: {}\n", last));
                    }
                }

                Ok(ToolExecutionResult::success(&output))
            }
            "enable" => {
                let name = args.get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'name' parameter".to_string()))?;

                let mut jobs = self.jobs.write().await;

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

                let mut jobs = self.jobs.write().await;

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

                let mut jobs = self.jobs.write().await;
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

                let jobs = self.jobs.read().await;

                if let Some(job) = jobs.iter().find(|j| j.name == name) {
                    // Execute the command immediately
                    let output = tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&job.command)
                        .output()
                        .await;

                    match output {
                        Ok(result) => {
                            let stdout = String::from_utf8_lossy(&result.stdout);
                            let stderr = String::from_utf8_lossy(&result.stderr);

                            let mut output_text = format!("🔄 Manually running cron job '{}'\n", name);
                            output_text.push_str(&format!("Command: {}\n\n", job.command));

                            if result.status.success() {
                                output_text.push_str(&format!("✅ Success!\nOutput:\n{}", stdout));
                            } else {
                                output_text.push_str(&format!("❌ Failed!\nError:\n{}", stderr));
                            }

                            Ok(ToolExecutionResult::success(&output_text))
                        }
                        Err(e) => {
                            Ok(ToolExecutionResult::error(&format!("Failed to execute: {}", e)))
                        }
                    }
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

    #[test]
    fn test_matches_field_star() {
        assert!(matches_field("*", 5, 0, 59));
        assert!(matches_field("*", 0, 0, 59));
    }

    #[test]
    fn test_matches_field_step() {
        assert!(matches_field("*/5", 0, 0, 59));
        assert!(matches_field("*/5", 5, 0, 59));
        assert!(matches_field("*/5", 10, 0, 59));
        assert!(!matches_field("*/5", 3, 0, 59));
    }

    #[test]
    fn test_matches_field_exact() {
        assert!(matches_field("15", 15, 0, 59));
        assert!(!matches_field("15", 16, 0, 59));
    }
}
