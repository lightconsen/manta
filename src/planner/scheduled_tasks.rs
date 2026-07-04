//! Scheduled / periodic tasks — cron-like agent scheduling.
//!
//! The [`TaskScheduler`] manages [`ScheduledTask`]s that run at specific
//! times or on recurring intervals. It uses standard cron expressions for
//! maximum flexibility.
//!
//! # Example
//!
//! ```rust,no_run
//! use syscity::planner::scheduled_tasks::{TaskScheduler, ScheduledTask, Schedule};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut scheduler = TaskScheduler::new();
//! scheduler.add(ScheduledTask::new(
//!     "morning-summary",
//!     "Check email and summarize",
//!     Schedule::cron("0 9 * * 1-5"), // Weekdays at 9am
//!     vec![/* DesktopActions */],
//! )).await?;
//!
//! scheduler.start(Arc::new(|task| Box::pin(async move {
//!     println!("Running: {}", task.name);
//! }))).await?;
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::computer::DesktopAction;

/// A task that has been scheduled for future execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// Unique identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Description of what this task does.
    pub description: String,
    /// When and how often to run.
    pub schedule: Schedule,
    /// Actions to execute when the task fires.
    pub actions: Vec<DesktopAction>,
    /// Whether the task is currently enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether the task is currently running (transient, not persisted).
    #[serde(skip)]
    pub is_running: bool,
    /// Last execution timestamp (ISO 8601).
    #[serde(default)]
    pub last_run: Option<String>,
    /// Next scheduled execution timestamp (ISO 8601).
    #[serde(default)]
    pub next_run: Option<String>,
    /// Count of total executions.
    #[serde(default)]
    pub run_count: u32,
    /// Maximum number of times to run (0 = unlimited).
    #[serde(default)]
    pub max_runs: u32,
}

fn default_true() -> bool {
    true
}

impl ScheduledTask {
    /// Create a new scheduled task.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        schedule: Schedule,
        actions: Vec<DesktopAction>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            schedule,
            actions,
            enabled: true,
            is_running: false,
            last_run: None,
            next_run: None,
            run_count: 0,
            max_runs: 0,
        }
    }

    /// Set a description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// Set maximum run count.
    pub fn with_max_runs(mut self, max: u32) -> Self {
        self.max_runs = max;
        self
    }

    /// Compute the next run time from now.
    pub fn compute_next_run(&self) -> Option<DateTime<Utc>> {
        if !self.enabled {
            return None;
        }
        if self.max_runs > 0 && self.run_count >= self.max_runs {
            return None;
        }
        self.schedule.next_after(Utc::now())
    }

    /// Mark as just executed.
    pub fn mark_run(&mut self) {
        self.last_run = Some(Utc::now().to_rfc3339());
        self.run_count += 1;
        self.next_run = self.compute_next_run().map(|d| d.to_rfc3339());
    }
}

/// Scheduling specification for a task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Schedule {
    /// Run once at the specified UTC timestamp (ISO 8601).
    Once(String),
    /// Run at fixed intervals (e.g. every 30 seconds).
    Interval {
        /// Interval duration in seconds.
        seconds: u64,
    },
    /// Cron expression (standard 5-field: minute hour day month dow).
    Cron {
        /// The cron expression string.
        expression: String,
    },
}

impl Schedule {
    /// Convenience: schedule that runs once at the given time.
    pub fn once(time: impl Into<String>) -> Self {
        Self::Once(time.into())
    }

    /// Convenience: fixed interval in seconds.
    pub fn interval(seconds: u64) -> Self {
        Self::Interval { seconds }
    }

    /// Convenience: standard 5-field cron expression.
    pub fn cron(expr: impl Into<String>) -> Self {
        Self::Cron { expression: expr.into() }
    }

    /// Compute the next occurrence after the given time.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Schedule::Once(time_str) => {
                let dt = DateTime::parse_from_rfc3339(time_str)
                    .ok()?
                    .with_timezone(&Utc);
                if dt > after {
                    Some(dt)
                } else {
                    None // Already passed.
                }
            }
            Schedule::Interval { seconds } => {
                Some(after + chrono::Duration::seconds(*seconds as i64))
            }
            Schedule::Cron { expression } => {
                parse_cron(expression).and_then(|fields| next_cron_occurrence(&fields, after))
            }
        }
    }
}

/// Type alias for a task handler function.
pub type TaskHandler =
    Arc<dyn Fn(ScheduledTask) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// In-memory task scheduler with cron support.
pub struct TaskScheduler {
    tasks: Arc<RwLock<HashMap<String, ScheduledTask>>>,
    running: Arc<AtomicBool>,
    handle: Option<tokio::task::JoinHandle<()>>,
    poll_interval_secs: u64,
}

impl TaskScheduler {
    /// Create a new empty scheduler.
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
            poll_interval_secs: 1,
        }
    }

    /// Set the poll interval in seconds (default: 1).
    pub fn with_poll_interval(mut self, secs: u64) -> Self {
        self.poll_interval_secs = secs;
        self
    }

    /// Add or replace a scheduled task.
    pub async fn add(&self, task: ScheduledTask) -> crate::Result<()> {
        let mut tasks = self.tasks.write().await;
        info!("Scheduled task '{}' ({})", task.name, task.id);
        tasks.insert(task.id.clone(), task);
        Ok(())
    }

    /// Remove a scheduled task by ID.
    pub async fn remove(&self, task_id: &str) -> crate::Result<bool> {
        let mut tasks = self.tasks.write().await;
        Ok(tasks.remove(task_id).is_some())
    }

    /// Enable a task.
    pub async fn enable(&self, task_id: &str) -> crate::Result<bool> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.enabled = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Disable a task.
    pub async fn disable(&self, task_id: &str) -> crate::Result<bool> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(task_id) {
            task.enabled = false;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get a task by ID.
    pub async fn get(&self, task_id: &str) -> Option<ScheduledTask> {
        let tasks = self.tasks.read().await;
        tasks.get(task_id).cloned()
    }

    /// List all scheduled tasks.
    pub async fn list(&self) -> Vec<ScheduledTask> {
        let tasks = self.tasks.read().await;
        tasks.values().cloned().collect()
    }

    /// Start the scheduler loop.
    ///
    /// The provided `handler` is called for each task that becomes due.
    /// Run this in a background task (e.g. `tokio::spawn`).
    pub async fn start(&mut self, handler: TaskHandler) -> crate::Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Err(crate::error::SyscityError::Validation(
                "Scheduler already running".to_string(),
            ));
        }

        let interval_secs = self.poll_interval_secs;
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let tasks = self.tasks.clone();

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            while running.load(Ordering::SeqCst) {
                interval.tick().await;
                let now = Utc::now();
                let mut task_list = {
                    let lock = tasks.read().await;
                    lock.values().cloned().collect::<Vec<_>>()
                };

                for task in &mut task_list {
                    if !task.enabled || task.is_running {
                        continue;
                    }
                    if task.max_runs > 0 && task.run_count >= task.max_runs {
                        continue;
                    }

                    let should_run = match &task.schedule {
                        Schedule::Once(time_str) => {
                            if let Ok(dt) = DateTime::parse_from_rfc3339(time_str) {
                                let dt_utc = dt.with_timezone(&Utc);
                                dt_utc <= now && task.run_count == 0
                            } else {
                                false
                            }
                        }
                        Schedule::Interval { seconds } => {
                            match task
                                .last_run
                                .as_ref()
                                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                            {
                                Some(last) => {
                                    let elapsed =
                                        now.signed_duration_since(last.with_timezone(&Utc));
                                    elapsed.num_seconds() >= *seconds as i64
                                }
                                None => true, // Never run before.
                            }
                        }
                        Schedule::Cron { expression } => {
                            match parse_cron(expression) {
                                Some(fields) => {
                                    let next = next_cron_occurrence(&fields, now);
                                    match (
                                        next,
                                        task.last_run
                                            .as_ref()
                                            .and_then(|s| DateTime::parse_from_rfc3339(s).ok()),
                                    ) {
                                        (Some(next_time), Some(last)) => {
                                            // Run if we've crossed the scheduled boundary.
                                            let last_utc = last.with_timezone(&Utc);
                                            next_time <= now && last_utc < next_time
                                        }
                                        (Some(next_time), None) => next_time <= now,
                                        _ => false,
                                    }
                                }
                                None => {
                                    warn!("Invalid cron expression: {}", expression);
                                    false
                                }
                            }
                        }
                    };

                    if should_run {
                        // Mark running and update last_run.
                        {
                            let mut lock = tasks.write().await;
                            if let Some(t) = lock.get_mut(&task.id) {
                                if t.is_running {
                                    continue; // Another check already started
                                              // it.
                                }
                                t.is_running = true;
                            }
                        }

                        let task_clone = task.clone();
                        let tasks_clone = tasks.clone();
                        let handler_clone = handler.clone();

                        tokio::spawn(async move {
                            info!("Executing scheduled task '{}'", task_clone.name);
                            handler_clone(task_clone.clone()).await;

                            let mut lock = tasks_clone.write().await;
                            if let Some(t) = lock.get_mut(&task_clone.id) {
                                t.is_running = false;
                                t.mark_run();
                                info!(
                                    "Scheduled task '{}' completed (run #{})",
                                    t.name, t.run_count
                                );
                            }
                        });
                    }
                }
            }
        });

        self.handle = Some(handle);
        info!("Task scheduler started");
        Ok(())
    }

    /// Stop the scheduler loop.
    pub async fn stop(&mut self) -> crate::Result<()> {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("Scheduler task panicked: {}", e),
                Err(_) => warn!("Scheduler task did not stop within 5s timeout"),
            }
        }
        info!("Task scheduler stopped");
        Ok(())
    }

    /// Serialize all tasks to JSON.
    pub async fn export_json(&self) -> crate::Result<String> {
        let tasks = self.list().await;
        serde_json::to_string_pretty(&tasks).map_err(crate::error::SyscityError::Serialization)
    }

    /// Load tasks from JSON.
    pub async fn import_json(&self, json: &str) -> crate::Result<()> {
        let tasks: Vec<ScheduledTask> =
            serde_json::from_str(json).map_err(crate::error::SyscityError::Serialization)?;
        for task in tasks {
            self.add(task).await?;
        }
        Ok(())
    }
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Minimal cron parser (5-field: minute hour day month dow)
// ---------------------------------------------------------------------------

/// Parsed cron fields.
#[derive(Debug, Clone)]
struct CronFields {
    minutes: Vec<u8>,
    hours: Vec<u8>,
    days: Vec<u8>,
    months: Vec<u8>,
    weekdays: Vec<u8>, // 0 = Sunday.
}

fn parse_cron(expr: &str) -> Option<CronFields> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 5 {
        return None;
    }
    Some(CronFields {
        minutes: parse_field(parts[0], 0, 59)?,
        hours: parse_field(parts[1], 0, 23)?,
        days: parse_field(parts[2], 1, 31)?,
        months: parse_field(parts[3], 1, 12)?,
        weekdays: parse_field(parts[4], 0, 6)?,
    })
}

fn parse_field(field: &str, min: u8, max: u8) -> Option<Vec<u8>> {
    let mut values = Vec::new();
    for part in field.split(',') {
        if part == "*" {
            for v in min..=max {
                values.push(v);
            }
        } else if let Some((start, end)) = part.split_once('-') {
            let start: u8 = start.parse().ok()?;
            let end: u8 = end.parse().ok()?;
            for v in start..=end {
                if v >= min && v <= max {
                    values.push(v);
                }
            }
        } else if let Some((base, step)) = part.split_once("/") {
            let base_val = if base == "*" { min } else { base.parse().ok()? };
            let step_val: u8 = step.parse().ok()?;
            let mut v = base_val;
            while v <= max {
                values.push(v);
                v += step_val;
            }
        } else {
            let v: u8 = part.parse().ok()?;
            if v >= min && v <= max {
                values.push(v);
            }
        }
    }
    values.sort_unstable();
    values.dedup();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

/// Find the next cron occurrence strictly after `after`.
fn next_cron_occurrence(fields: &CronFields, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let mut candidate = after + chrono::Duration::minutes(1);
    candidate = candidate
        .with_second(0)
        .unwrap_or(candidate)
        .with_nanosecond(0)
        .unwrap_or(candidate);

    // Brute-force scan up to 4 years to avoid infinite loops.
    let limit = after + chrono::Duration::days(366 * 4);

    while candidate <= limit {
        if fields.months.contains(&(candidate.month() as u8))
            && fields.days.contains(&(candidate.day() as u8))
            && fields.hours.contains(&(candidate.hour() as u8))
            && fields.minutes.contains(&(candidate.minute() as u8))
            && fields
                .weekdays
                .contains(&(candidate.weekday().num_days_from_sunday() as u8))
        {
            return Some(candidate);
        }
        candidate += chrono::Duration::minutes(1);
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_once() {
        let future = Utc::now() + chrono::Duration::hours(1);
        let s = Schedule::once(future.to_rfc3339());
        let next = s.next_after(Utc::now());
        assert!(next.is_some());
    }

    #[test]
    fn test_schedule_once_past() {
        let past = Utc::now() - chrono::Duration::hours(1);
        let s = Schedule::once(past.to_rfc3339());
        let next = s.next_after(Utc::now());
        assert!(next.is_none());
    }

    #[test]
    fn test_schedule_interval() {
        let s = Schedule::interval(300);
        let now = Utc::now();
        let next = s.next_after(now).unwrap();
        let diff = next.signed_duration_since(now);
        assert!(diff.num_seconds() >= 300);
    }

    #[test]
    fn test_cron_parser() {
        let fields = parse_cron("0 9 * * 1-5").unwrap();
        assert_eq!(fields.minutes, vec![0]);
        assert_eq!(fields.hours, vec![9]);
        assert!(fields.days.len() == 31); // * -> all days.
        assert!(fields.months.len() == 12);
        assert_eq!(fields.weekdays, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_cron_next_occurrence() {
        let fields = parse_cron("0 9 * * *").unwrap();
        let now = Utc::now();
        let next = next_cron_occurrence(&fields, now).unwrap();
        assert_eq!(next.minute(), 0);
        assert_eq!(next.hour(), 9);
    }

    #[test]
    fn test_cron_step() {
        let fields = parse_cron("*/15 * * * *").unwrap();
        assert_eq!(fields.minutes, vec![0, 15, 30, 45]);
    }

    #[test]
    fn test_scheduled_task_max_runs() {
        let mut task =
            ScheduledTask::new("t", "test", Schedule::interval(60), vec![]).with_max_runs(2);
        assert!(task.compute_next_run().is_some());
        task.mark_run();
        task.mark_run();
        assert!(task.compute_next_run().is_none());
    }

    #[test]
    fn test_parse_field_star() {
        let vals = parse_field("*", 0, 5).unwrap();
        assert_eq!(vals, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_parse_field_range() {
        let vals = parse_field("1-3", 0, 5).unwrap();
        assert_eq!(vals, vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_field_step() {
        let vals = parse_field("*/2", 0, 5).unwrap();
        assert_eq!(vals, vec![0, 2, 4]);
    }

    #[tokio::test]
    async fn test_scheduler_add_remove() {
        let scheduler = TaskScheduler::new();
        let task = ScheduledTask::new("t1", "test", Schedule::interval(60), vec![]);
        scheduler.add(task).await.unwrap();
        assert!(scheduler.get("t1").await.is_some());
        assert!(scheduler.remove("t1").await.unwrap());
        assert!(scheduler.get("t1").await.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_enable_disable() {
        let scheduler = TaskScheduler::new();
        let task = ScheduledTask::new("t1", "test", Schedule::interval(60), vec![]);
        scheduler.add(task).await.unwrap();
        scheduler.disable("t1").await.unwrap();
        let t = scheduler.get("t1").await.unwrap();
        assert!(!t.enabled);
        scheduler.enable("t1").await.unwrap();
        let t = scheduler.get("t1").await.unwrap();
        assert!(t.enabled);
    }

    #[tokio::test]
    async fn test_scheduler_export_import() {
        let scheduler = TaskScheduler::new();
        let task = ScheduledTask::new("t1", "test", Schedule::cron("0 9 * * *"), vec![])
            .with_description("morning task");
        scheduler.add(task).await.unwrap();

        let json = scheduler.export_json().await.unwrap();
        let scheduler2 = TaskScheduler::new();
        scheduler2.import_json(&json).await.unwrap();
        let tasks = scheduler2.list().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "test");
        assert_eq!(tasks[0].schedule, Schedule::cron("0 9 * * *"));
    }
}
