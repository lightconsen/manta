//! Advanced Cron Scheduler for Manta
//!
//! Production-grade scheduler supporting AI agent execution, multi-channel delivery,
//! and enterprise reliability features.

use crate::agent::Agent;
use crate::channels::IncomingMessage;
use crate::error::{MantaError, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use cron::Schedule as CronSchedule;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, error, info, warn};

/// Delivery event emitted for `DeliveryMode::Announce` jobs.
///
/// The gateway (or any consumer) receives these on the channel returned by
/// `CronScheduler::with_announce_tx` and forwards them to the
/// appropriate messaging channel (Discord, Telegram, WhatsApp, etc.).
#[derive(Debug, Clone)]
pub struct AnnounceDelivery {
    /// Target channel name (e.g. `"discord"`, `"telegram"`)
    pub channel: String,
    /// Recipient / room identifier (e.g. channel ID or user handle)
    pub to: String,
    /// Message content to deliver
    pub message: String,
}

/// Execution target - what to run
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ExecutionTarget {
    /// Execute shell command
    Shell { command: String },
    /// Execute via AI agent
    Agent {
        agent_id: Option<String>,
        prompt: String,
        context: Option<String>,
    },
}

impl ExecutionTarget {
    /// Create a shell execution target
    pub fn shell(command: impl Into<String>) -> Self {
        Self::Shell { command: command.into() }
    }

    /// Create an agent execution target
    pub fn agent(prompt: impl Into<String>) -> Self {
        Self::Agent {
            agent_id: None,
            prompt: prompt.into(),
            context: None,
        }
    }

    /// Create an agent execution target with specific agent
    pub fn agent_with_id(agent_id: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self::Agent {
            agent_id: Some(agent_id.into()),
            prompt: prompt.into(),
            context: None,
        }
    }
}

/// Session target - where to execute
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionTarget {
    /// Run in main session (has conversation context)
    Main,
    /// Run in isolated session (clean state: cron:{job_id})
    Isolated,
}

impl Default for SessionTarget {
    fn default() -> Self {
        Self::Isolated
    }
}

/// Delivery mode for job results
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum DeliveryMode {
    /// No delivery (fire-and-forget)
    None,
    /// Send to messaging channel
    Announce { channel: String, to: String },
    /// POST to webhook URL
    Webhook {
        url: String,
        headers: HashMap<String, String>,
    },
}

/// Schedule types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Schedule {
    /// Run at a specific time (one-shot)
    At { timestamp: DateTime<Utc> },
    /// Run every N seconds
    Every {
        interval: Duration,
        anchor: Option<DateTime<Utc>>,
    },
    /// Cron expression
    Cron {
        expression: String,
        timezone: Option<String>,
        stagger_ms: Option<u64>,
    },
}

impl Schedule {
    /// Calculate the next run time after `from`
    pub fn next_run(&self, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Schedule::At { timestamp } => {
                if *timestamp > from {
                    Some(*timestamp)
                } else {
                    None
                }
            }
            Schedule::Every { interval, anchor } => {
                let anchor = anchor.unwrap_or(from);

                if from < anchor {
                    Some(anchor)
                } else {
                    let elapsed = from.signed_duration_since(anchor);
                    let periods = (elapsed.num_seconds() / interval.as_secs() as i64) + 1;
                    Some(anchor + ChronoDuration::seconds(periods * interval.as_secs() as i64))
                }
            }
            Schedule::Cron {
                expression,
                timezone: _,
                stagger_ms,
            } => {
                // Parse cron expression
                // The cron crate v0.14 expects 6 fields (with seconds), so we need to
                // convert 5-field expressions to 6-field by prepending "0" for seconds
                let normalized = if expression.trim().split_whitespace().count() == 5 {
                    format!("0 {}", expression.trim())
                } else {
                    expression.clone()
                };

                let schedule = CronSchedule::from_str(&normalized).ok()?;

                // Get next occurrence
                let next = schedule.upcoming(Utc).next()?;

                // Add stagger if configured
                if let Some(stagger) = stagger_ms {
                    let jitter = rand::random::<u64>() % stagger;
                    Some(next + ChronoDuration::milliseconds(jitter as i64))
                } else {
                    Some(next)
                }
            }
        }
    }

    /// Check if this is a one-shot schedule that should be deleted after execution
    pub fn is_one_shot(&self) -> bool {
        matches!(self, Schedule::At { .. })
    }
}

/// Backoff strategy for retries
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BackoffStrategy {
    /// Fixed delay between retries
    Fixed,
    /// Exponential backoff
    Exponential,
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub backoff: BackoffStrategy,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            backoff: BackoffStrategy::Exponential,
        }
    }
}

impl RetryConfig {
    /// Calculate delay for a specific retry attempt
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_delay_secs = match self.backoff {
            BackoffStrategy::Fixed => 30,
            BackoffStrategy::Exponential => {
                // Tiered exponential: 30s, 1m, 5m, 15m, 1h
                match attempt {
                    0 => 30,
                    1 => 60,
                    2 => 300,
                    3 => 900,
                    _ => 3600,
                }
            }
        };
        Duration::from_secs(base_delay_secs)
    }
}

/// Job execution state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobState {
    /// When the job is scheduled to run next
    pub next_run_at: Option<DateTime<Utc>>,
    /// When the job last ran
    pub last_run_at: Option<DateTime<Utc>>,
    /// When the job started running (if currently running)
    pub running_at_ms: Option<i64>,
    /// Total execution count
    pub run_count: u32,
    /// Last error message
    pub last_error: Option<String>,
    /// Consecutive error count
    pub consecutive_errors: u32,
}

/// A cron job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub schedule: Schedule,
    pub target: ExecutionTarget,
    pub session: SessionTarget,
    pub delivery: DeliveryMode,
    pub retry: RetryConfig,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub state: JobState,
}

impl CronJob {
    /// Create a new cron job
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        schedule: Schedule,
        target: ExecutionTarget,
    ) -> Self {
        let now = Utc::now();
        let next_run = schedule.next_run(now);

        Self {
            id: id.into(),
            name: name.into(),
            schedule,
            target,
            session: SessionTarget::default(),
            delivery: DeliveryMode::None,
            retry: RetryConfig::default(),
            enabled: true,
            created_at: now,
            state: JobState {
                next_run_at: next_run,
                ..Default::default()
            },
        }
    }

    /// Set the delivery mode
    pub fn with_delivery(mut self, delivery: DeliveryMode) -> Self {
        self.delivery = delivery;
        self
    }

    /// Set the session target
    pub fn with_session(mut self, session: SessionTarget) -> Self {
        self.session = session;
        self
    }

    /// Set retry configuration
    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    /// Check if the job should run at the given time
    pub fn should_run(&self, now: DateTime<Utc>) -> bool {
        if !self.enabled {
            return false;
        }

        if self.state.running_at_ms.is_some() {
            return false;
        }

        match self.state.next_run_at {
            Some(next) => now >= next,
            None => true,
        }
    }

    /// Update the next run time based on schedule
    pub fn update_next_run(&mut self, after: DateTime<Utc>) {
        self.state.next_run_at = self.schedule.next_run(after);
        if let Some(next) = self.state.next_run_at {
            debug!("Job {} next run at {}", self.name, next);
        }
    }
}

/// Run status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    Error,
}

/// Delivery status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Delivered,
    Failed(String),
}

/// Run log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunLogEntry {
    pub run_id: String,
    pub job_id: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: RunStatus,
    pub output: Option<String>,
    pub error: Option<String>,
    pub delivery_status: Option<DeliveryStatus>,
}

/// Maximum timer delay - wake at least once per minute to check for schedule changes
const MAX_TIMER_DELAY_MS: u64 = 60_000;
/// Minimum delay between timer fires to prevent tight loops
const MIN_REFIRE_GAP_MS: u64 = 2_000;

/// Commands for the scheduler
#[derive(Debug)]
pub enum CronCommand {
    Add(CronJob),
    Remove(String),
    SetEnabled(String, bool),
    Trigger(String),
    GetNextRun(String, oneshot::Sender<Option<DateTime<Utc>>>),
    ListJobs(oneshot::Sender<Vec<CronJob>>),
    GetJob(String, oneshot::Sender<Option<CronJob>>),
}

/// Advanced cron scheduler with single global timer (OpenClaw-style)
pub struct CronScheduler {
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    command_tx: mpsc::Sender<CronCommand>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// Shared agent reference — wrapping in `Arc<RwLock<…>>` lets
    /// `set_agent()` update the running scheduler's agent without
    /// restarting any background tasks.
    agent: Arc<RwLock<Option<Arc<Agent>>>>,
    store_path: Option<PathBuf>,
    /// Optional sender for Announce-mode delivery events.
    announce_tx: Option<mpsc::Sender<AnnounceDelivery>>,
    /// Notify the timer to re-calculate next wake time
    rearm_notify: Arc<tokio::sync::Notify>,
}

impl std::fmt::Debug for CronScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CronScheduler")
            .field("store_path", &self.store_path)
            .field("has_announce_tx", &self.announce_tx.is_some())
            .finish()
    }
}

impl CronScheduler {
    /// Create a new scheduler
    pub fn new() -> (Self, mpsc::Receiver<CronCommand>) {
        let (command_tx, command_rx) = mpsc::channel(100);
        let scheduler = Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            command_tx,
            shutdown_tx: None,
            agent: Arc::new(RwLock::new(None)),
            store_path: None,
            announce_tx: None,
            rearm_notify: Arc::new(tokio::sync::Notify::new()),
        };
        (scheduler, command_rx)
    }

    /// Attach an announce delivery sender.
    ///
    /// When a cron job uses `DeliveryMode::Announce`, the scheduler sends an
    /// [`AnnounceDelivery`] event on this channel.  The caller is responsible
    /// for receiving the events and routing them to the correct messaging back-end.
    pub fn set_announce_tx(&mut self, tx: mpsc::Sender<AnnounceDelivery>) {
        self.announce_tx = Some(tx);
    }

    /// Wire an `Agent` into a running scheduler.
    ///
    /// Because all background tasks hold an `Arc` to the same
    /// `RwLock<Option<Arc<Agent>>>`, calling this after `start()` is safe
    /// and immediately visible to any task that tries to execute an agent job.
    pub async fn set_agent(&self, agent: Arc<Agent>) {
        *self.agent.write().await = Some(agent);
    }

    /// Set the store path for persistence
    pub fn with_store_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.store_path = Some(path.into());
        self
    }

    /// Start the scheduler
    pub async fn start(&mut self, mut command_rx: mpsc::Receiver<CronCommand>) -> Result<()> {
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        // Load jobs from store if configured
        let store_path = self.store_path.clone();
        if let Some(ref path) = store_path {
            self.load_jobs(path).await.ok();
        }

        let jobs = Arc::clone(&self.jobs);
        let agent = Arc::clone(&self.agent);
        let store_path = self.store_path.clone();
        let announce_tx = self.announce_tx.clone();

        // Spawn command handler
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    cmd = command_rx.recv() => {
                        if let Some(cmd) = cmd {
                            Self::handle_command(&jobs, &agent, &store_path, &announce_tx, cmd).await;
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Cron scheduler shutting down");
                        break;
                    }
                }
            }
        });

        // Spawn single global timer task (OpenClaw-style)
        let jobs_for_timer = Arc::clone(&self.jobs);
        let agent_for_timer = Arc::clone(&self.agent);
        let store_path_for_timer = self.store_path.clone();
        let announce_tx_for_timer = self.announce_tx.clone();
        let rearm_notify = Arc::clone(&self.rearm_notify);

        tokio::spawn(async move {
            // Track if we're currently running jobs to prevent overlapping ticks
            let running = Arc::new(RwLock::new(false));

            loop {
                // Calculate next wake time (minimum delay across all jobs)
                let delay_ms = Self::calculate_next_wake_ms(&jobs_for_timer).await;

                // Cap at MAX_TIMER_DELAY_MS to ensure we wake at least once per minute
                let capped_delay = delay_ms
                    .map(|d| d.min(MAX_TIMER_DELAY_MS))
                    .unwrap_or(MAX_TIMER_DELAY_MS);

                // Ensure minimum delay to prevent tight loops
                let final_delay = capped_delay.max(MIN_REFIRE_GAP_MS);

                debug!(
                    "Timer armed: delay={}ms (capped={}, min={})",
                    delay_ms.unwrap_or(u64::MAX),
                    capped_delay,
                    final_delay
                );

                // Wait for timer OR rearm notification
                let sleep_fut = tokio::time::sleep(Duration::from_millis(final_delay));
                let notify_fut = rearm_notify.notified();

                tokio::select! {
                    _ = sleep_fut => {
                        // Timer fired - proceed to check jobs
                    }
                    _ = notify_fut => {
                        debug!("Timer re-arming due to schedule change");
                        continue; // Recalculate immediately
                    }
                }

                // Check if already running (prevent overlapping ticks like OpenClaw's armRunningRecheckTimer)
                let running_guard = running.read().await;
                if *running_guard {
                    debug!("Timer tick skipped: previous tick still running");
                    continue; // Will re-arm with recalculated delay
                }
                drop(running_guard);

                // Mark as running
                *running.write().await = true;

                // Run due jobs - ALWAYS re-arm in finally pattern
                let jobs = Arc::clone(&jobs_for_timer);
                let agent = Arc::clone(&agent_for_timer);
                let store_path = store_path_for_timer.clone();
                let announce_tx = announce_tx_for_timer.clone();

                // Run jobs (result unused)
                async {
                    Self::run_due_jobs(&jobs, &agent, &store_path, &announce_tx).await;
                }
                .await;

                // Always mark as not running and continue (re-arm happens at loop start)
                *running.write().await = false;

                // The loop continues and re-arms automatically
            }
        });

        info!("Cron scheduler started (single global timer)");
        Ok(())
    }

    /// Calculate the next wake time in milliseconds
    /// Returns None if no jobs are scheduled
    async fn calculate_next_wake_ms(jobs: &Arc<RwLock<HashMap<String, CronJob>>>) -> Option<u64> {
        let jobs_lock = jobs.read().await;
        let now = Utc::now();
        let now_ms = now.timestamp_millis() as u64;

        let mut min_next_ms: Option<u64> = None;

        for (_, job) in jobs_lock.iter() {
            if !job.enabled || job.state.running_at_ms.is_some() {
                continue;
            }
            if let Some(next_run) = job.state.next_run_at {
                let next_ms = next_run.timestamp_millis() as u64;
                if next_ms > now_ms {
                    let delay = next_ms - now_ms;
                    if min_next_ms.map(|m| delay < m).unwrap_or(true) {
                        min_next_ms = Some(delay);
                    }
                } else {
                    // Job is overdue - wake immediately
                    return Some(0);
                }
            }
        }

        min_next_ms
    }

    /// Run all jobs that are currently due
    async fn run_due_jobs(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        agent: &Arc<RwLock<Option<Arc<Agent>>>>,
        store_path: &Option<PathBuf>,
        announce_tx: &Option<mpsc::Sender<AnnounceDelivery>>,
    ) {
        let due_job_ids: Vec<String> = {
            let jobs_lock = jobs.read().await;
            let now = Utc::now();

            jobs_lock
                .iter()
                .filter_map(|(id, job)| {
                    if job.should_run(now) {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };

        if due_job_ids.is_empty() {
            return;
        }

        info!("Running {} due cron jobs", due_job_ids.len());

        for job_id in due_job_ids {
            Self::execute_job(jobs, &job_id, agent, store_path, announce_tx).await;
        }
    }

    /// Trigger timer rearm after schedule changes
    fn trigger_rearm(&self) {
        self.rearm_notify.notify_one();
    }

    /// Handle scheduler commands
    async fn handle_command(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        agent: &Arc<RwLock<Option<Arc<Agent>>>>,
        store_path: &Option<PathBuf>,
        announce_tx: &Option<mpsc::Sender<AnnounceDelivery>>,
        cmd: CronCommand,
    ) {
        match cmd {
            CronCommand::Add(mut job) => {
                info!("Adding job: {} ({})", job.name, job.id);

                // Calculate initial next run
                if job.state.next_run_at.is_none() {
                    job.update_next_run(Utc::now());
                }

                jobs.write().await.insert(job.id.clone(), job);

                // Persist
                if let Some(ref path) = store_path {
                    let _ = Self::save_jobs(jobs, path).await;
                }
            }
            CronCommand::Remove(id) => {
                info!("Removing job: {}", id);
                jobs.write().await.remove(&id);

                if let Some(ref path) = store_path {
                    let _ = Self::save_jobs(jobs, path).await;
                }
            }
            CronCommand::SetEnabled(id, enabled) => {
                let mut jobs_lock = jobs.write().await;
                if let Some(job) = jobs_lock.get_mut(&id) {
                    job.enabled = enabled;
                    info!("Job {} enabled = {}", id, enabled);

                    // Recalculate next run if enabling
                    if enabled {
                        job.update_next_run(Utc::now());
                    }
                }
                drop(jobs_lock);

                if let Some(ref path) = store_path {
                    let _ = Self::save_jobs(jobs, path).await;
                }
            }
            CronCommand::Trigger(id) => {
                info!("Triggering job: {}", id);
                Self::execute_job(jobs, &id, agent, store_path, announce_tx).await;
            }
            CronCommand::GetNextRun(id, tx) => {
                let jobs_lock = jobs.read().await;
                let next = jobs_lock.get(&id).and_then(|j| j.state.next_run_at);
                let _ = tx.send(next);
            }
            CronCommand::ListJobs(tx) => {
                let jobs_lock = jobs.read().await;
                let list: Vec<CronJob> = jobs_lock.values().cloned().collect();
                let _ = tx.send(list);
            }
            CronCommand::GetJob(id, tx) => {
                let jobs_lock = jobs.read().await;
                let job = jobs_lock.get(&id).cloned();
                let _ = tx.send(job);
            }
        }
    }

    /// Execute a job
    async fn execute_job(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        job_id: &str,
        agent: &Arc<RwLock<Option<Arc<Agent>>>>,
        store_path: &Option<PathBuf>,
        announce_tx: &Option<mpsc::Sender<AnnounceDelivery>>,
    ) {
        let job = {
            let mut jobs_lock = jobs.write().await;
            let job = match jobs_lock.get_mut(job_id) {
                Some(j) => j,
                None => {
                    warn!("Job {} not found for execution", job_id);
                    return;
                }
            };

            // Check if should run
            let now = Utc::now();
            if !job.should_run(now) {
                return;
            }

            // Mark as running
            job.state.running_at_ms = Some(now.timestamp_millis());
            job.clone()
        };

        info!("Executing job: {}", job.name);
        let run_id = uuid::Uuid::new_v4().to_string();
        let started_at = Utc::now();

        // Execute based on target type
        let result = match &job.target {
            ExecutionTarget::Shell { command } => Self::execute_shell(command).await,
            ExecutionTarget::Agent { prompt, agent_id, .. } => {
                let agent_guard = agent.read().await;
                if let Some(ref agent_ref) = *agent_guard {
                    Self::execute_agent(agent_ref, &job, prompt, agent_id.as_deref()).await
                } else {
                    Err(MantaError::Internal("No agent configured for cron job".to_string()))
                }
            }
        };

        let completed_at = Utc::now();

        // Update job state
        {
            let mut jobs_lock = jobs.write().await;
            if let Some(j) = jobs_lock.get_mut(job_id) {
                j.state.running_at_ms = None;
                j.state.last_run_at = Some(completed_at);
                j.state.run_count += 1;

                match &result {
                    Ok(output) => {
                        j.state.last_error = None;
                        j.state.consecutive_errors = 0;
                        info!("Job '{}' completed successfully", j.name);

                        // Deliver result if configured
                        if !matches!(j.delivery, DeliveryMode::None) {
                            if let Err(e) =
                                Self::deliver_result(&j.delivery, output, announce_tx).await
                            {
                                warn!("Delivery failed for job '{}': {}", j.name, e);
                            }
                        }
                        // Success - update next run normally
                        j.update_next_run(completed_at);
                    }
                    Err(e) => {
                        let error_msg = format!("{}", e);
                        j.state.last_error = Some(error_msg.clone());
                        j.state.consecutive_errors += 1;
                        error!("Job '{}' failed: {}", j.name, error_msg);

                        // Check if we should retry
                        if j.state.consecutive_errors <= j.retry.max_retries {
                            let delay = j.retry.delay_for_attempt(j.state.consecutive_errors);
                            let retry_at = completed_at
                                + chrono::Duration::from_std(delay)
                                    .unwrap_or_else(|_| chrono::Duration::seconds(60));
                            warn!("Scheduling retry for job '{}' at {:?}", j.name, retry_at);
                            j.state.next_run_at = Some(retry_at);
                        } else {
                            // Max retries exceeded - update next run normally
                            j.update_next_run(completed_at);
                        }
                    }
                }

                // Remove one-shot jobs after execution
                if j.schedule.is_one_shot() {
                    info!("Removing one-shot job: {}", j.name);
                    jobs_lock.remove(job_id);
                }
            }
        }

        // Persist
        if let Some(ref path) = store_path {
            let _ = Self::save_jobs(jobs, path).await;
        }

        // Log the run
        let _ = Self::log_run(job_id, &run_id, started_at, completed_at, result, store_path).await;
    }

    /// Execute shell command
    async fn execute_shell(command: &str) -> Result<String> {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to execute shell: {}", e)))?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(MantaError::Internal(format!("Shell error: {}", stderr)))
        }
    }

    /// Execute via agent
    async fn execute_agent(
        agent: &Arc<Agent>,
        job: &CronJob,
        prompt: &str,
        _agent_id: Option<&str>,
    ) -> Result<String> {
        let session_id = match job.session {
            SessionTarget::Main => "cron:main".to_string(),
            SessionTarget::Isolated => format!("cron:{}", job.id),
        };

        let message = IncomingMessage::new("system", &session_id, prompt)
            .with_provenance(crate::channels::InputProvenance::InternalSystem {
                source: "cron".to_string(),
            })
            .with_metadata(
                crate::channels::MessageMetadata::new()
                    .with_extra("job_id", job.id.clone())
                    .with_extra("job_name", job.name.clone()),
            );

        let response = agent.process_message(message).await?;
        Ok(response.content)
    }

    /// Deliver result
    async fn deliver_result(
        delivery: &DeliveryMode,
        output: &str,
        announce_tx: &Option<mpsc::Sender<AnnounceDelivery>>,
    ) -> Result<()> {
        match delivery {
            DeliveryMode::None => Ok(()),
            DeliveryMode::Announce { channel, to } => {
                info!("Delivering result to {}:{}", channel, to);
                if let Some(tx) = announce_tx {
                    tx.send(AnnounceDelivery {
                        channel: channel.clone(),
                        to: to.clone(),
                        message: output.to_string(),
                    })
                    .await
                    .map_err(|_| MantaError::Internal("Announce channel closed".to_string()))?;
                } else {
                    debug!(
                        "No announce_tx configured; output: {}",
                        output.chars().take(100).collect::<String>()
                    );
                }
                Ok(())
            }
            DeliveryMode::Webhook { url, headers } => {
                info!("Delivering result to webhook: {}", url);

                let client = reqwest::Client::new();
                let mut request = client.post(url).body(output.to_string());

                for (key, value) in headers {
                    request = request.header(key, value);
                }

                request.send().await.map_err(|e| MantaError::Http(e))?;

                Ok(())
            }
        }
    }

    /// Log a job run
    async fn log_run(
        job_id: &str,
        run_id: &str,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        result: Result<String>,
        store_path: &Option<PathBuf>,
    ) -> Result<()> {
        let entry = match result {
            Ok(output) => RunLogEntry {
                run_id: run_id.to_string(),
                job_id: job_id.to_string(),
                started_at,
                completed_at: Some(completed_at),
                status: RunStatus::Ok,
                output: Some(output),
                error: None,
                delivery_status: Some(DeliveryStatus::Delivered),
            },
            Err(e) => RunLogEntry {
                run_id: run_id.to_string(),
                job_id: job_id.to_string(),
                started_at,
                completed_at: Some(completed_at),
                status: RunStatus::Error,
                output: None,
                error: Some(format!("{}", e)),
                delivery_status: Some(DeliveryStatus::Failed("Execution error".to_string())),
            },
        };

        debug!("Job run logged: {} - {:?}", entry.job_id, entry.status);

        // Persist to JSONL file if store_path is configured
        if let Some(ref path) = store_path {
            let log_path = path.with_extension("runs.jsonl");
            let line = serde_json::to_string(&entry)
                .map_err(|e| MantaError::Internal(format!("Failed to serialize run log: {}", e)))?;
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .await
                .map_err(|e| MantaError::Internal(format!("Failed to open run log: {}", e)))?;
            use tokio::io::AsyncWriteExt;
            file.write_all(line.as_bytes())
                .await
                .map_err(|e| MantaError::Internal(format!("Failed to write run log: {}", e)))?;
            file.write_all(b"\n")
                .await
                .map_err(|e| MantaError::Internal(format!("Failed to write run log: {}", e)))?;
        }

        Ok(())
    }

    /// Load jobs from store
    async fn load_jobs(&mut self, path: &PathBuf) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to read jobs file: {}", e)))?;

        let jobs: Vec<CronJob> = serde_json::from_str(&content)
            .map_err(|e| MantaError::Internal(format!("Failed to parse jobs: {}", e)))?;

        let mut jobs_lock = self.jobs.write().await;
        for job in jobs {
            // Clear stale running markers (crash recovery)
            let mut job = job;
            if job.state.running_at_ms.is_some() {
                job.state.running_at_ms = None;
                job.state.last_error = Some("Recovered from crash".to_string());
            }

            jobs_lock.insert(job.id.clone(), job);
        }

        info!("Loaded {} jobs from store", jobs_lock.len());
        Ok(())
    }

    /// Save jobs to store
    async fn save_jobs(jobs: &Arc<RwLock<HashMap<String, CronJob>>>, path: &PathBuf) -> Result<()> {
        let jobs_lock = jobs.read().await;
        let jobs_vec: Vec<&CronJob> = jobs_lock.values().collect();

        let json = serde_json::to_string_pretty(&jobs_vec)
            .map_err(|e| MantaError::Internal(format!("Failed to serialize jobs: {}", e)))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        tokio::fs::write(path, json)
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to write jobs file: {}", e)))?;

        Ok(())
    }

    /// Public API methods

    /// Add a job
    pub async fn add_job(&self, job: CronJob) -> Result<()> {
        self.command_tx
            .send(CronCommand::Add(job))
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to add job: {}", e)))?;
        // Trigger rearm to pick up new job
        self.trigger_rearm();
        Ok(())
    }

    /// Remove a job
    pub async fn remove_job(&self, job_id: &str) -> Result<()> {
        self.command_tx
            .send(CronCommand::Remove(job_id.to_string()))
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to remove job: {}", e)))?;
        self.trigger_rearm();
        Ok(())
    }

    /// Enable/disable a job
    pub async fn set_job_enabled(&self, job_id: &str, enabled: bool) -> Result<()> {
        self.command_tx
            .send(CronCommand::SetEnabled(job_id.to_string(), enabled))
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to set job state: {}", e)))?;
        if enabled {
            self.trigger_rearm();
        }
        Ok(())
    }

    /// Trigger a job immediately
    pub async fn trigger_job(&self, job_id: &str) -> Result<()> {
        self.command_tx
            .send(CronCommand::Trigger(job_id.to_string()))
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to trigger job: {}", e)))
    }

    /// List all jobs
    pub async fn list_jobs(&self) -> Vec<CronJob> {
        let (tx, rx) = oneshot::channel();
        let _ = self.command_tx.send(CronCommand::ListJobs(tx)).await;
        rx.await.unwrap_or_default()
    }

    /// Get a specific job
    pub async fn get_job(&self, job_id: &str) -> Option<CronJob> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(CronCommand::GetJob(job_id.to_string(), tx))
            .await;
        rx.await.ok().flatten()
    }

    /// Shutdown the scheduler
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
        Ok(())
    }
}

impl Default for CronScheduler {
    fn default() -> Self {
        let (scheduler, _) = Self::new();
        scheduler
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_next_run_at() {
        let now = Utc::now();
        let future = now + ChronoDuration::hours(1);

        let schedule = Schedule::At { timestamp: future };
        assert_eq!(schedule.next_run(now), Some(future));

        // Past time returns None
        let past = now - ChronoDuration::hours(1);
        let schedule = Schedule::At { timestamp: past };
        assert_eq!(schedule.next_run(now), None);
    }

    #[test]
    fn test_schedule_next_run_every() {
        let now = Utc::now();
        let interval = Duration::from_secs(3600); // 1 hour

        let schedule = Schedule::Every { interval, anchor: None };

        let next = schedule.next_run(now);
        assert!(next.is_some());

        // Should be about 1 hour from now
        let diff = next.unwrap() - now;
        assert!(diff.num_seconds() >= 3600);
    }

    #[test]
    fn test_execution_target_creation() {
        let shell = ExecutionTarget::shell("echo hello");
        assert!(matches!(shell, ExecutionTarget::Shell { command } if command == "echo hello"));

        let agent = ExecutionTarget::agent("summarize");
        assert!(matches!(agent, ExecutionTarget::Agent { prompt, .. } if prompt == "summarize"));
    }

    #[test]
    fn test_backoff_delay() {
        let retry = RetryConfig {
            max_retries: 5,
            backoff: BackoffStrategy::Exponential,
        };

        assert_eq!(retry.delay_for_attempt(0).as_secs(), 30);
        assert_eq!(retry.delay_for_attempt(1).as_secs(), 60);
        assert_eq!(retry.delay_for_attempt(2).as_secs(), 300);
        assert_eq!(retry.delay_for_attempt(3).as_secs(), 900);
        assert_eq!(retry.delay_for_attempt(4).as_secs(), 3600);
        assert_eq!(retry.delay_for_attempt(10).as_secs(), 3600); // Capped at tier 4

        let fixed = RetryConfig {
            max_retries: 5,
            backoff: BackoffStrategy::Fixed,
        };
        assert_eq!(fixed.delay_for_attempt(0).as_secs(), 30);
        assert_eq!(fixed.delay_for_attempt(5).as_secs(), 30);
    }

    #[test]
    fn test_schedule_cron_expression() {
        let now = Utc::now();
        // Every minute expression (6 fields with seconds)
        let schedule = Schedule::Cron {
            expression: "0 * * * * *".to_string(),
            timezone: None,
            stagger_ms: None,
        };
        let next = schedule.next_run(now);
        assert!(next.is_some());
        let next = next.unwrap();
        assert!(next > now);
        // Should be within ~1 minute
        assert!((next - now).num_seconds() <= 61);
    }

    #[test]
    fn test_schedule_cron_5field_normalization() {
        let now = Utc::now();
        // 5-field cron (no seconds) should be normalized to 6-field
        let schedule = Schedule::Cron {
            expression: "* * * * *".to_string(),
            timezone: None,
            stagger_ms: None,
        };
        let next = schedule.next_run(now);
        assert!(next.is_some());
        assert!(next.unwrap() > now);
    }

    #[test]
    fn test_schedule_is_one_shot() {
        let future = Utc::now() + ChronoDuration::hours(1);
        let at = Schedule::At { timestamp: future };
        assert!(at.is_one_shot());

        let every = Schedule::Every {
            interval: Duration::from_secs(60),
            anchor: None,
        };
        assert!(!every.is_one_shot());

        let cron = Schedule::Cron {
            expression: "0 * * * * *".to_string(),
            timezone: None,
            stagger_ms: None,
        };
        assert!(!cron.is_one_shot());
    }

    #[test]
    fn test_execution_target_agent_with_id() {
        let target = ExecutionTarget::agent_with_id("agent-1", "do something");
        assert!(matches!(
            target,
            ExecutionTarget::Agent {
                agent_id: Some(id),
                prompt,
                context: None,
            } if id == "agent-1" && prompt == "do something"
        ));
    }

    #[test]
    fn test_session_target_default() {
        assert_eq!(SessionTarget::default(), SessionTarget::Isolated);
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.backoff, BackoffStrategy::Exponential);
    }

    #[test]
    fn test_job_state_default() {
        let state = JobState::default();
        assert!(state.next_run_at.is_none());
        assert!(state.last_run_at.is_none());
        assert!(state.running_at_ms.is_none());
        assert_eq!(state.run_count, 0);
        assert!(state.last_error.is_none());
        assert_eq!(state.consecutive_errors, 0);
    }

    #[test]
    fn test_cron_job_new() {
        let future = Utc::now() + ChronoDuration::hours(1);
        let schedule = Schedule::At { timestamp: future };
        let target = ExecutionTarget::shell("echo hello");
        let job = CronJob::new("job-1", "Test Job", schedule.clone(), target);

        assert_eq!(job.id, "job-1");
        assert_eq!(job.name, "Test Job");
        assert_eq!(job.schedule, schedule);
        assert!(job.enabled);
        assert_eq!(job.session, SessionTarget::Isolated);
        assert!(matches!(job.delivery, DeliveryMode::None));
        assert_eq!(job.retry.max_retries, 3);
        assert!(job.state.next_run_at.is_some());
    }

    #[test]
    fn test_cron_job_builder_methods() {
        let future = Utc::now() + ChronoDuration::hours(1);
        let job = CronJob::new(
            "job-2",
            "Builder Test",
            Schedule::At { timestamp: future },
            ExecutionTarget::agent("test"),
        )
        .with_delivery(DeliveryMode::Announce {
            channel: "discord".to_string(),
            to: "#general".to_string(),
        })
        .with_session(SessionTarget::Main)
        .with_retry(RetryConfig {
            max_retries: 5,
            backoff: BackoffStrategy::Fixed,
        });

        assert!(matches!(
            job.delivery,
            DeliveryMode::Announce { channel, to } if channel == "discord" && to == "#general"
        ));
        assert_eq!(job.session, SessionTarget::Main);
        assert_eq!(job.retry.max_retries, 5);
        assert_eq!(job.retry.backoff, BackoffStrategy::Fixed);
    }

    #[test]
    fn test_cron_job_should_run() {
        let now = Utc::now();
        let past = now - ChronoDuration::minutes(5);
        let schedule = Schedule::At { timestamp: past };
        let target = ExecutionTarget::shell("echo");

        // Job with past schedule - should run because next_run is past and enabled
        let mut job = CronJob::new("j1", "Test", schedule, target);
        job.state.next_run_at = Some(past);
        assert!(job.should_run(now));

        // Disabled job should not run
        job.enabled = false;
        assert!(!job.should_run(now));
        job.enabled = true;

        // Running job should not run
        job.state.running_at_ms = Some(now.timestamp_millis());
        assert!(!job.should_run(now));
        job.state.running_at_ms = None;

        // Future job should not run yet
        let future = now + ChronoDuration::hours(1);
        job.state.next_run_at = Some(future);
        assert!(!job.should_run(now));

        // No next_run should still return true (legacy behavior)
        job.state.next_run_at = None;
        assert!(job.should_run(now));
    }

    #[test]
    fn test_cron_job_update_next_run() {
        let now = Utc::now();
        let interval = Duration::from_secs(3600);
        let schedule = Schedule::Every { interval, anchor: Some(now) };
        let mut job = CronJob::new("j2", "Interval", schedule, ExecutionTarget::shell("echo"));

        job.update_next_run(now);
        let next = job.state.next_run_at.unwrap();
        // Next run should be about 1 hour from anchor
        let diff = next - now;
        assert!(diff.num_seconds() >= 3600);
    }

    #[test]
    fn test_run_status_variants() {
        assert_eq!(RunStatus::Ok, RunStatus::Ok);
        assert_eq!(RunStatus::Error, RunStatus::Error);
        assert_ne!(RunStatus::Ok, RunStatus::Error);
    }

    #[test]
    fn test_delivery_status_variants() {
        assert_eq!(DeliveryStatus::Delivered, DeliveryStatus::Delivered);
        assert_eq!(
            DeliveryStatus::Failed("x".to_string()),
            DeliveryStatus::Failed("x".to_string())
        );
        assert_ne!(DeliveryStatus::Delivered, DeliveryStatus::Failed("x".to_string()));
    }

    #[test]
    fn test_announce_delivery_creation() {
        let ann = AnnounceDelivery {
            channel: "telegram".to_string(),
            to: "12345".to_string(),
            message: "hello".to_string(),
        };
        assert_eq!(ann.channel, "telegram");
        assert_eq!(ann.to, "12345");
        assert_eq!(ann.message, "hello");
    }

    #[test]
    fn test_cron_scheduler_new() {
        let (scheduler, rx) = CronScheduler::new();
        assert!(scheduler.store_path.is_none());
        assert!(scheduler.announce_tx.is_none());
        // rx should be open
        assert!(!rx.is_closed());
    }

    #[test]
    fn test_cron_scheduler_default() {
        let scheduler: CronScheduler = Default::default();
        assert!(scheduler.store_path.is_none());
    }

    #[test]
    fn test_cron_command_variants() {
        let job = CronJob::new(
            "id",
            "name",
            Schedule::At { timestamp: Utc::now() },
            ExecutionTarget::shell("echo"),
        );

        let add = CronCommand::Add(job.clone());
        assert!(matches!(add, CronCommand::Add(_)));

        let remove = CronCommand::Remove("id".to_string());
        assert!(matches!(remove, CronCommand::Remove(s) if s == "id"));

        let set_enabled = CronCommand::SetEnabled("id".to_string(), false);
        assert!(matches!(set_enabled, CronCommand::SetEnabled(s, false) if s == "id"));

        let trigger = CronCommand::Trigger("id".to_string());
        assert!(matches!(trigger, CronCommand::Trigger(s) if s == "id"));

        let (tx, _rx) = oneshot::channel();
        let get_next = CronCommand::GetNextRun("id".to_string(), tx);
        assert!(matches!(get_next, CronCommand::GetNextRun(s, _) if s == "id"));

        let (tx, _rx) = oneshot::channel();
        let list = CronCommand::ListJobs(tx);
        assert!(matches!(list, CronCommand::ListJobs(_)));

        let (tx, _rx) = oneshot::channel();
        let get = CronCommand::GetJob("id".to_string(), tx);
        assert!(matches!(get, CronCommand::GetJob(s, _) if s == "id"));
    }
}
