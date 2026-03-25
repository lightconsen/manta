//! Cron Scheduler for Manta
//!
//! This module provides types and utilities for scheduled task execution.
//! The actual scheduling is handled by the `advanced` submodule which provides
//! a production-grade scheduler with timer-based execution, retry logic,
//! crash recovery, and run history logging.
//!
//! # Architecture
//!
//! - **CronScheduler** (`advanced`): The primary scheduler implementation
//!   used by the Gateway and CronTool
//! - **ScheduledJob**: Legacy job type kept for backward compatibility
//!
//! # Deprecated
//!
//! The legacy `CronScheduler` has been removed. All cron functionality now
//! goes through `CronScheduler`.

pub mod advanced;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A scheduled job
///
/// This is the legacy job structure. New code should use `CronJob`
/// from the `advanced` module instead.
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

/// Parse a cron expression and calculate next run time
pub fn calculate_next_run(schedule: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    // Handle common shorthand expressions
    match schedule.trim().to_lowercase().as_str() {
        "@hourly" => Some(from + chrono::Duration::hours(1)),
        "@daily" => Some(from + chrono::Duration::days(1)),
        "@weekly" => Some(from + chrono::Duration::weeks(1)),
        "@monthly" => Some(from + chrono::Duration::days(30)),
        expr => {
            // Try to parse as standard cron
            parse_cron_expression(expr, from)
        }
    }
}

/// Parse a standard 5-field cron expression using the `cron` crate.
fn parse_cron_expression(expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    use std::str::FromStr;

    match cron::Schedule::from_str(expr) {
        Ok(schedule) => schedule.after(&from).next(),
        Err(e) => {
            tracing::warn!("Invalid cron expression '{}': {}", expr, e);
            None
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduled_job() {
        let mut job = ScheduledJob::new("job1", "Test Job", "@hourly", "Run diagnostics", "cli");

        assert!(job.enabled);
        assert_eq!(job.run_count, 0);

        let now = Utc::now();
        job.mark_executed(now);

        assert_eq!(job.run_count, 1);
        assert!(job.last_run.is_some());
    }

    #[test]
    fn test_job_max_runs() {
        let mut job = ScheduledJob::new("job1", "Test", "@hourly", "test", "cli").with_max_runs(2);

        let now = Utc::now();

        // First run - should run because next_run is None
        assert!(job.should_run(now));
        job.mark_executed(now);

        // Second run - use a future time after the next scheduled run
        let future = now + chrono::Duration::hours(2);
        assert!(job.should_run(future));
        job.mark_executed(future);

        // After max runs reached, should not run even in the future
        let far_future = future + chrono::Duration::hours(2);
        assert!(!job.should_run(far_future));
    }

    #[test]
    fn test_natural_language_parsing() {
        assert_eq!(parse_natural_language("every hour"), Some("@hourly".to_string()));
        assert_eq!(parse_natural_language("daily"), Some("@daily".to_string()));
        assert_eq!(parse_natural_language("run weekly"), Some("@weekly".to_string()));
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
