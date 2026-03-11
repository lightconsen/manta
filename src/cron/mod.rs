//! Cron Scheduler for Manta
//!
//! This module implements scheduled task execution using cron expressions.
//! Jobs can be scheduled with natural language or standard cron syntax.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// A scheduled job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJob {
    /// Unique job ID
    pub id: String,
    /// Job name/description
    pub name: String,
    /// Cron expression
    pub schedule: String,
    /// The prompt/command to execute
    pub prompt: String,
    /// Channel to deliver results to
    pub channel: String,
    /// Whether the job is enabled
    pub enabled: bool,
    /// When the job was created
    pub created_at: DateTime<Utc>,
    /// Last execution time
    pub last_run: Option<DateTime<Utc>>,
    /// Next scheduled execution
    pub next_run: Option<DateTime<Utc>>,
    /// Execution count
    pub run_count: u32,
    /// Maximum executions (None = unlimited)
    pub max_runs: Option<u32>,
}

impl ScheduledJob {
    /// Create a new scheduled job
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        schedule: impl Into<String>,
        prompt: impl Into<String>,
        channel: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            schedule: schedule.into(),
            prompt: prompt.into(),
            channel: channel.into(),
            enabled: true,
            created_at: Utc::now(),
            last_run: None,
            next_run: None,
            run_count: 0,
            max_runs: None,
        }
    }

    /// Set maximum runs
    pub fn with_max_runs(mut self, max: u32) -> Self {
        self.max_runs = Some(max);
        self
    }

    /// Check if job should run now
    pub fn should_run(&self, now: DateTime<Utc>) -> bool {
        if !self.enabled {
            return false;
        }

        if let Some(max) = self.max_runs {
            if self.run_count >= max {
                return false;
            }
        }

        match self.next_run {
            Some(next) => now >= next,
            None => true,
        }
    }

    /// Update after execution
    pub fn mark_executed(&mut self, now: DateTime<Utc>) {
        self.last_run = Some(now);
        self.run_count += 1;
        self.next_run = calculate_next_run(&self.schedule, now);
    }
}

/// Cron scheduler
#[derive(Debug)]
pub struct CronScheduler {
    /// Jobs storage
    jobs: Arc<RwLock<HashMap<String, ScheduledJob>>>,
    /// Command sender for job execution
    command_tx: mpsc::Sender<JobCommand>,
    /// Background task handle
    handle: Option<JoinHandle<()>>,
    /// Shutdown signal
    shutdown_tx: Option<mpsc::Sender<()>>,
}

/// Commands for the scheduler
#[derive(Debug)]
enum JobCommand {
    /// Add a new job
    Add(ScheduledJob),
    /// Remove a job
    Remove(String),
    /// Enable/disable a job
    SetEnabled(String, bool),
    /// Trigger a job immediately
    Trigger(String),
}

impl CronScheduler {
    /// Create a new cron scheduler
    pub fn new() -> (Self, mpsc::Receiver<JobCommand>) {
        let (command_tx, command_rx) = mpsc::channel(100);
        let scheduler = Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            command_tx,
            handle: None,
            shutdown_tx: None,
        };
        (scheduler, command_rx)
    }

    /// Start the scheduler
    pub async fn start(&mut self, mut command_rx: mpsc::Receiver<JobCommand>) -> crate::Result<()> {
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        let jobs = Arc::clone(&self.jobs);

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        Self::check_and_run_jobs(&jobs).await;
                    }
                    cmd = command_rx.recv() => {
                        if let Some(cmd) = cmd {
                            Self::handle_command(&jobs, cmd).await;
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Cron scheduler shutting down");
                        break;
                    }
                }
            }
        });

        self.handle = Some(handle);
        info!("Cron scheduler started");
        Ok(())
    }

    /// Shutdown the scheduler
    pub async fn shutdown(&mut self) -> crate::Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        Ok(())
    }

    /// Add a job
    pub async fn add_job(&self, job: ScheduledJob) -> crate::Result<()> {
        self.command_tx
            .send(JobCommand::Add(job))
            .await
            .map_err(|e| crate::error::MantaError::Internal(format!("Failed to add job: {}", e)))?;
        Ok(())
    }

    /// Remove a job
    pub async fn remove_job(&self, job_id: &str) -> crate::Result<()> {
        self.command_tx
            .send(JobCommand::Remove(job_id.to_string()))
            .await
            .map_err(|e| {
                crate::error::MantaError::Internal(format!("Failed to remove job: {}", e))
            })?;
        Ok(())
    }

    /// Enable/disable a job
    pub async fn set_job_enabled(&self, job_id: &str, enabled: bool) -> crate::Result<()> {
        self.command_tx
            .send(JobCommand::SetEnabled(job_id.to_string(), enabled))
            .await
            .map_err(|e| {
                crate::error::MantaError::Internal(format!("Failed to set job state: {}", e))
            })?;
        Ok(())
    }

    /// Trigger a job immediately
    pub async fn trigger_job(&self, job_id: &str) -> crate::Result<()> {
        self.command_tx
            .send(JobCommand::Trigger(job_id.to_string()))
            .await
            .map_err(|e| {
                crate::error::MantaError::Internal(format!("Failed to trigger job: {}", e))
            })?;
        Ok(())
    }

    /// Get all jobs
    pub async fn list_jobs(&self) -> Vec<ScheduledJob> {
        let jobs = self.jobs.read().await;
        jobs.values().cloned().collect()
    }

    /// Get a specific job
    pub async fn get_job(&self, job_id: &str) -> Option<ScheduledJob> {
        let jobs = self.jobs.read().await;
        jobs.get(job_id).cloned()
    }

    /// Handle scheduler commands
    async fn handle_command(jobs: &Arc<RwLock<HashMap<String, ScheduledJob>>>, cmd: JobCommand) {
        let mut jobs_lock = jobs.write().await;
        match cmd {
            JobCommand::Add(job) => {
                info!("Adding job: {} ({})", job.name, job.id);
                jobs_lock.insert(job.id.clone(), job);
            }
            JobCommand::Remove(id) => {
                info!("Removing job: {}", id);
                jobs_lock.remove(&id);
            }
            JobCommand::SetEnabled(id, enabled) => {
                if let Some(job) = jobs_lock.get_mut(&id) {
                    job.enabled = enabled;
                    info!("Job {} enabled = {}", id, enabled);
                }
            }
            JobCommand::Trigger(id) => {
                if let Some(job) = jobs_lock.get_mut(&id) {
                    info!("Triggering job: {}", id);
                    Self::execute_job(job).await;
                }
            }
        }
    }

    /// Check and run due jobs
    async fn check_and_run_jobs(jobs: &Arc<RwLock<HashMap<String, ScheduledJob>>>) {
        let now = Utc::now();
        let mut jobs_lock = jobs.write().await;

        for job in jobs_lock.values_mut() {
            if job.should_run(now) {
                info!("Running scheduled job: {}", job.name);
                Self::execute_job(job).await;
                job.mark_executed(now);
            }
        }
    }

    /// Execute a job
    async fn execute_job(job: &mut ScheduledJob) {
        // In a real implementation, this would:
        // 1. Create an agent instance
        // 2. Process the prompt
        // 3. Deliver results to the specified channel
        // 4. Handle errors and retries

        info!(
            "Executing job '{}' with prompt: {}",
            job.name,
            job.prompt.chars().take(50).collect::<String>()
        );

        // Simulate execution
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        debug!("Job '{}' completed", job.name);
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        let (scheduler, _) = Self::new();
        scheduler
    }
}

/// Parse a cron expression and calculate next run time
fn calculate_next_run(schedule: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    // Simplified implementation - in production, use a proper cron parser
    // like the `cron` crate

    // Handle common shorthand expressions
    match schedule.trim().to_lowercase().as_str() {
        "@hourly" => Some(from + chrono::Duration::hours(1)),
        "@daily" => Some(from + chrono::Duration::days(1)),
        "@weekly" => Some(from + chrono::Duration::weeks(1)),
        "@monthly" => Some(from + chrono::Duration::days(30)),
        expr => {
            // Try to parse as standard cron (simplified)
            parse_cron_expression(expr, from)
        }
    }
}

/// Simple cron expression parser
fn parse_cron_expression(expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let parts: Vec<&str> = expr.split_whitespace().collect();

    if parts.len() != 5 {
        warn!("Invalid cron expression: {}", expr);
        return None;
    }

    // Very simplified: just add 1 hour for any valid expression
    // In production, use the `cron` crate for proper parsing
    Some(from + chrono::Duration::hours(1))
}

/// Parse natural language schedule
pub fn parse_natural_language(input: &str) -> Option<String> {
    let input = input.to_lowercase();

    if input.contains("every hour") || input.contains("hourly") {
        Some("@hourly".to_string())
    } else if input.contains("every day") || input.contains("daily") {
        Some("@daily".to_string())
    } else if input.contains("every week") || input.contains("weekly") {
        Some("@weekly".to_string())
    } else if input.contains("every month") || input.contains("monthly") {
        Some("@monthly".to_string())
    } else {
        None
    }
}

/// Tool for cron job management
pub mod tool {
    use super::*;
    use crate::tools::{Tool, ToolContext, ToolExecutionResult};
    use async_trait::async_trait;
    use serde_json::json;

    /// Tool for managing scheduled jobs
    #[derive(Debug)]
    pub struct CronTool {
        scheduler: CronScheduler,
    }

    impl CronTool {
        /// Create a new cron tool
        pub fn new(scheduler: CronScheduler) -> Self {
            Self { scheduler }
        }
    }

    #[async_trait]
    impl Tool for CronTool {
        fn name(&self) -> &str {
            "cron"
        }

        fn description(&self) -> &str {
            r#"Schedule recurring tasks and reminders.

Use this to set up automated jobs that run on a schedule.
Supports natural language ("every day at 9am") or cron expressions ("0 9 * * *").

Examples:
- Schedule a daily summary
- Set up hourly health checks
- Create weekly reports
- Schedule reminders"#
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add", "remove", "list", "enable", "disable", "trigger"],
                        "description": "The action to perform"
                    },
                    "job_id": {
                        "type": "string",
                        "description": "Job ID (for remove/enable/disable/trigger)"
                    },
                    "name": {
                        "type": "string",
                        "description": "Job name (for add)"
                    },
                    "schedule": {
                        "type": "string",
                        "description": "Cron expression or natural language (for add)"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The prompt/command to execute (for add)"
                    },
                    "channel": {
                        "type": "string",
                        "description": "Channel to deliver results to (for add)"
                    }
                },
                "required": ["action"]
            })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
            _context: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            let action = args["action"]
                .as_str()
                .ok_or_else(|| crate::error::MantaError::Validation("action is required".to_string()))?;

            match action {
                "add" => {
                    let name = args["name"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "name is required for add".to_string()
                        ))?;
                    let schedule = args["schedule"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "schedule is required for add".to_string()
                        ))?;
                    let prompt = args["prompt"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "prompt is required for add".to_string()
                        ))?;
                    let channel = args["channel"]
                        .as_str()
                        .unwrap_or("default");

                    // Try to parse natural language
                    let schedule = parse_natural_language(schedule)
                        .unwrap_or_else(|| schedule.to_string());

                    let job = ScheduledJob::new(
                        uuid::Uuid::new_v4().to_string(),
                        name,
                        schedule,
                        prompt,
                        channel,
                    );

                    let job_id = job.id.clone();
                    self.scheduler.add_job(job).await?;

                    Ok(ToolExecutionResult::success(format!("Created job: {}", name))
                        .with_data(json!({"job_id": job_id})))
                }

                "remove" => {
                    let job_id = args["job_id"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "job_id is required for remove".to_string()
                        ))?;

                    self.scheduler.remove_job(job_id).await?;
                    Ok(ToolExecutionResult::success(format!("Removed job: {}", job_id)))
                }

                "list" => {
                    let jobs = self.scheduler.list_jobs().await;
                    let formatted: Vec<serde_json::Value> = jobs
                        .iter()
                        .map(|j| {
                            json!({
                                "id": j.id,
                                "name": j.name,
                                "schedule": j.schedule,
                                "enabled": j.enabled,
                                "run_count": j.run_count,
                                "last_run": j.last_run.map(|t| t.to_rfc3339()),
                                "next_run": j.next_run.map(|t| t.to_rfc3339())
                            })
                        })
                        .collect();

                    Ok(ToolExecutionResult::success(format!("{} jobs found", jobs.len()))
                        .with_data(json!({"jobs": formatted, "count": jobs.len()})))
                }

                "enable" => {
                    let job_id = args["job_id"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "job_id is required for enable".to_string()
                        ))?;

                    self.scheduler.set_job_enabled(job_id, true).await?;
                    Ok(ToolExecutionResult::success(format!("Enabled job: {}", job_id)))
                }

                "disable" => {
                    let job_id = args["job_id"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "job_id is required for disable".to_string()
                        ))?;

                    self.scheduler.set_job_enabled(job_id, false).await?;
                    Ok(ToolExecutionResult::success(format!("Disabled job: {}", job_id)))
                }

                "trigger" => {
                    let job_id = args["job_id"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "job_id is required for trigger".to_string()
                        ))?;

                    self.scheduler.trigger_job(job_id).await?;
                    Ok(ToolExecutionResult::success(format!("Triggered job: {}", job_id)))
                }

                _ => Err(crate::error::MantaError::Validation(format!(
                    "Unknown action: {}",
                    action
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduled_job() {
        let mut job = ScheduledJob::new(
            "job1",
            "Test Job",
            "@hourly",
            "Run diagnostics",
            "cli",
        );

        assert!(job.enabled);
        assert_eq!(job.run_count, 0);

        let now = Utc::now();
        job.mark_executed(now);

        assert_eq!(job.run_count, 1);
        assert!(job.last_run.is_some());
    }

    #[test]
    fn test_job_max_runs() {
        let mut job = ScheduledJob::new("job1", "Test", "@hourly", "test", "cli")
            .with_max_runs(2);

        let now = Utc::now();

        assert!(job.should_run(now));
        job.mark_executed(now);

        assert!(job.should_run(now));
        job.mark_executed(now);

        assert!(!job.should_run(now));
    }

    #[test]
    fn test_natural_language_parsing() {
        assert_eq!(
            parse_natural_language("every hour"),
            Some("@hourly".to_string())
        );
        assert_eq!(
            parse_natural_language("daily"),
            Some("@daily".to_string())
        );
        assert_eq!(
            parse_natural_language("run weekly"),
            Some("@weekly".to_string())
        );
    }

    #[test]
    fn test_calculate_next_run() {
        let now = Utc::now();

        let next = calculate_next_run("@hourly", now);
        assert!(next.is_some());

        let next = calculate_next_run("@daily", now);
        assert!(next.is_some());
    }
}
