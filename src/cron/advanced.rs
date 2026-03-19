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
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

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
        Self::Shell {
            command: command.into(),
        }
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
    Announce {
        channel: String,
        to: String,
    },
    /// POST to webhook URL
    Webhook {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

impl Default for DeliveryMode {
    fn default() -> Self {
        Self::None
    }
}

/// Schedule types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Schedule {
    /// One-shot execution at specific time
    At { timestamp: DateTime<Utc> },
    /// Fixed interval
    Every {
        #[serde(with = "duration_secs")]
        interval: Duration,
        anchor: Option<DateTime<Utc>>,
    },
    /// Cron expression (5 or 6 field)
    Cron {
        expression: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stagger_ms: Option<u64>,
    },
}

/// Duration serialization helper
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

impl Schedule {
    /// Calculate next run time from given timestamp
    pub fn next_run(&self, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Schedule::At { timestamp } => {
                if *timestamp > from {
                    Some(*timestamp)
                } else {
                    None // One-shot in the past
                }
            }
            Schedule::Every { interval, anchor } => {
                let interval = ChronoDuration::from_std(*interval).ok()?;
                let anchor = anchor.unwrap_or(from);

                if from < anchor {
                    Some(anchor)
                } else {
                    let elapsed = from.signed_duration_since(anchor);
                    let periods = (elapsed.num_seconds() / interval.num_seconds()) + 1;
                    Some(anchor + interval * periods as i32)
                }
            }
            Schedule::Cron {
                expression,
                timezone: _,
                stagger_ms,
            } => {
                // Parse cron expression
                let schedule = CronSchedule::from_str(expression).ok()?;

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
    /// Linear increasing delay
    Linear,
    /// Exponential backoff
    Exponential,
}

impl Default for BackoffStrategy {
    fn default() -> Self {
        Self::Exponential
    }
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u32,
    #[serde(default)]
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
        let tier = attempt.min(4) as usize;

        let base_ms = match self.backoff {
            BackoffStrategy::Fixed => BACKOFF_TIERS[0],
            BackoffStrategy::Linear => BACKOFF_TIERS[tier],
            BackoffStrategy::Exponential => BACKOFF_TIERS[tier],
        };

        Duration::from_millis(base_ms)
    }
}

/// Exponential backoff tiers
const BACKOFF_TIERS: [u64; 5] = [
    30_000,   // 30s - 1st error
    60_000,   // 1m - 2nd error
    300_000,  // 5m - 3rd error
    900_000,  // 15m - 4th error
    3_600_000, // 1h - 5th+ error
];

/// Job state for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobState {
    pub running_at_ms: Option<i64>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub run_count: u32,
    pub consecutive_errors: u32,
}

impl Default for JobState {
    fn default() -> Self {
        Self {
            running_at_ms: None,
            last_run_at: None,
            last_error: None,
            next_run_at: None,
            run_count: 0,
            consecutive_errors: 0,
        }
    }
}

/// An advanced scheduled job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedCronJob {
    /// Unique job ID
    pub id: String,
    /// Job name/description
    pub name: String,
    /// Schedule definition
    pub schedule: Schedule,
    /// What to execute
    pub target: ExecutionTarget,
    /// Where to execute
    #[serde(default)]
    pub session: SessionTarget,
    /// How to deliver results
    #[serde(default)]
    pub delivery: DeliveryMode,
    /// Retry configuration
    #[serde(default)]
    pub retry: RetryConfig,
    /// Whether the job is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// When the job was created
    pub created_at: DateTime<Utc>,
    /// Maximum executions (None = unlimited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_runs: Option<u32>,
    /// Job state
    #[serde(default)]
    pub state: JobState,
}

fn default_true() -> bool {
    true
}

impl AdvancedCronJob {
    /// Create a new scheduled job
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        schedule: Schedule,
        target: ExecutionTarget,
    ) -> Self {
        let id = id.into();
        Self {
            name: name.into(),
            schedule,
            target,
            session: SessionTarget::default(),
            delivery: DeliveryMode::default(),
            retry: RetryConfig::default(),
            enabled: true,
            created_at: Utc::now(),
            max_runs: None,
            id,
            state: JobState::default(),
        }
    }

    /// Set session target
    pub fn with_session(mut self, session: SessionTarget) -> Self {
        self.session = session;
        self
    }

    /// Set delivery mode
    pub fn with_delivery(mut self, delivery: DeliveryMode) -> Self {
        self.delivery = delivery;
        self
    }

    /// Set retry configuration
    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
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

        // Check if already running
        if self.state.running_at_ms.is_some() {
            return false;
        }

        // Check max runs
        if let Some(max) = self.max_runs {
            if self.state.run_count >= max {
                return false;
            }
        }

        // Check schedule
        match self.state.next_run_at {
            Some(next) => now >= next,
            None => true,
        }
    }

    /// Update next run time
    pub fn update_next_run(&mut self, now: DateTime<Utc>) {
        self.state.next_run_at = self.schedule.next_run(now);
    }
}

/// Run status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    Error,
    Cancelled,
}

/// Delivery status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Pending,
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

/// Commands for the scheduler
#[derive(Debug)]
pub enum CronCommand {
    Add(AdvancedCronJob),
    Remove(String),
    SetEnabled(String, bool),
    Trigger(String),
    GetNextRun(String, tokio::sync::oneshot::Sender<Option<DateTime<Utc>>>),
    ListJobs(tokio::sync::oneshot::Sender<Vec<AdvancedCronJob>>),
    GetJob(String, tokio::sync::oneshot::Sender<Option<AdvancedCronJob>>),
}

/// Advanced cron scheduler with timer-based execution
pub struct AdvancedCronScheduler {
    jobs: Arc<RwLock<HashMap<String, AdvancedCronJob>>>,
    command_tx: mpsc::Sender<CronCommand>,
    timer_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    agent: Option<Arc<Agent>>,
    store_path: Option<PathBuf>,
}

impl std::fmt::Debug for AdvancedCronScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdvancedCronScheduler")
            .field("jobs", &self.jobs)
            .field("has_timer", &self.timer_handle.is_some())
            .field("has_agent", &self.agent.is_some())
            .field("store_path", &self.store_path)
            .finish()
    }
}

impl AdvancedCronScheduler {
    /// Create a new scheduler
    pub fn new() -> (Self, mpsc::Receiver<CronCommand>) {
        let (command_tx, command_rx) = mpsc::channel(100);
        let scheduler = Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            command_tx,
            timer_handle: None,
            shutdown_tx: None,
            agent: None,
            store_path: None,
        };
        (scheduler, command_rx)
    }

    /// Create a new scheduler with agent
    pub fn with_agent(agent: Arc<Agent>) -> (Self, mpsc::Receiver<CronCommand>) {
        let (command_tx, command_rx) = mpsc::channel(100);
        let scheduler = Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
            command_tx,
            timer_handle: None,
            shutdown_tx: None,
            agent: Some(agent),
            store_path: None,
        };
        (scheduler, command_rx)
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
        let agent = self.agent.clone();
        let store_path = self.store_path.clone();

        // Arm initial timer
        self.arm_timer().await?;

        // Spawn command handler
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    cmd = command_rx.recv() => {
                        if let Some(cmd) = cmd {
                            Self::handle_command(&jobs, agent.as_ref(), &store_path, cmd).await;
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Advanced cron scheduler shutting down");
                        break;
                    }
                }
            }
        });

        self.timer_handle = Some(handle);
        info!("Advanced cron scheduler started");
        Ok(())
    }

    /// Arm the timer for the next job
    async fn arm_timer(&mut self) -> Result<()> {
        let jobs = self.jobs.read().await;
        let now = Utc::now();

        // Find the next job to run
        let mut next_job: Option<(String, DateTime<Utc>)> = None;

        for (id, job) in jobs.iter() {
            if !job.enabled || job.state.running_at_ms.is_some() {
                continue;
            }

            if let Some(next_run) = job.state.next_run_at {
                if next_job.is_none() || next_run < next_job.as_ref().unwrap().1 {
                    next_job = Some((id.clone(), next_run));
                }
            }
        }

        drop(jobs);

        // If we have a timer handle, abort it and create a new one
        if let Some(handle) = self.timer_handle.take() {
            handle.abort();
        }

        if let Some((job_id, run_at)) = next_job {
            let delay = run_at.signed_duration_since(now);

            if delay.num_seconds() > 0 {
                let jobs = Arc::clone(&self.jobs);
                let agent = self.agent.clone();
                let store_path = self.store_path.clone();
                let job_id_clone = job_id.clone();

                let handle = tokio::spawn(async move {
                    sleep(delay.to_std().unwrap_or(Duration::from_secs(1))).await;

                    // Execute the job
                    let mut jobs_lock = jobs.write().await;
                    if let Some(job) = jobs_lock.get_mut(&job_id_clone) {
                        if job.should_run(Utc::now()) {
                            drop(jobs_lock);
                            Self::execute_job(&jobs, &job_id_clone, agent.as_ref(), &store_path).await;
                        }
                    }
                });

                self.timer_handle = Some(handle);
                debug!("Timer armed for job {} at {}", job_id, run_at);
            } else {
                // Job is overdue, run immediately
                let jobs = Arc::clone(&self.jobs);
                let agent = self.agent.clone();
                let store_path = self.store_path.clone();
                let job_id_clone = job_id.clone();

                let handle = tokio::spawn(async move {
                    Self::execute_job(&jobs, &job_id_clone, agent.as_ref(), &store_path).await;
                });

                self.timer_handle = Some(handle);
            }
        }

        Ok(())
    }

    /// Handle scheduler commands
    async fn handle_command(
        jobs: &Arc<RwLock<HashMap<String, AdvancedCronJob>>>,
        agent: Option<&Arc<Agent>>,
        store_path: &Option<PathBuf>,
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
                Self::execute_job(jobs, &id, agent, store_path).await;
            }
            CronCommand::GetNextRun(id, tx) => {
                let jobs_lock = jobs.read().await;
                let next = jobs_lock.get(&id).and_then(|j| j.state.next_run_at);
                let _ = tx.send(next);
            }
            CronCommand::ListJobs(tx) => {
                let jobs_lock = jobs.read().await;
                let list: Vec<AdvancedCronJob> = jobs_lock.values().cloned().collect();
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
        jobs: &Arc<RwLock<HashMap<String, AdvancedCronJob>>>,
        job_id: &str,
        agent: Option<&Arc<Agent>>,
        store_path: &Option<PathBuf>,
    ) {
        let mut job = {
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
            ExecutionTarget::Shell { command } => {
                Self::execute_shell(command).await
            }
            ExecutionTarget::Agent { prompt, agent_id, .. } => {
                if let Some(agent) = agent {
                    Self::execute_agent(agent, &job, prompt, agent_id.as_deref()).await
                } else {
                    Err(MantaError::Internal("No agent configured".to_string()))
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
                            Self::deliver_result(&j.delivery, output).await;
                        }
                    }
                    Err(e) => {
                        let error_msg = format!("{}", e);
                        j.state.last_error = Some(error_msg.clone());
                        j.state.consecutive_errors += 1;
                        error!("Job '{}' failed: {}", j.name, error_msg);

                        // Check if we should retry
                        if j.state.consecutive_errors <= j.retry.max_retries {
                            let delay = j.retry.delay_for_attempt(j.state.consecutive_errors);
                            warn!("Will retry job '{}' in {:?}", j.name, delay);
                            // TODO: Schedule retry
                        }
                    }
                }

                // Update next run time
                j.update_next_run(completed_at);

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
        let _ = Self::log_run(job_id, &run_id, started_at, completed_at, result).await;
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
        job: &AdvancedCronJob,
        prompt: &str,
        _agent_id: Option<&str>,
    ) -> Result<String> {
        let session_id = match job.session {
            SessionTarget::Main => "cron:main".to_string(),
            SessionTarget::Isolated => format!("cron:{}", job.id),
        };

        let message = IncomingMessage::new("system", &session_id, prompt)
            .with_metadata(
                crate::channels::MessageMetadata::new()
                    .with_extra("job_id", job.id.clone())
                    .with_extra("job_name", job.name.clone()),
            );

        let response = agent.process_message(message).await?;
        Ok(response.content)
    }

    /// Deliver result
    async fn deliver_result(delivery: &DeliveryMode, output: &str) -> Result<()> {
        match delivery {
            DeliveryMode::None => Ok(()),
            DeliveryMode::Announce { channel, to } => {
                info!("Delivering result to {}:{}", channel, to);
                // TODO: Integrate with channel system
                debug!("Output: {}", output.chars().take(100).collect::<String>());
                Ok(())
            }
            DeliveryMode::Webhook { url, headers } => {
                info!("Delivering result to webhook: {}", url);

                let client = reqwest::Client::new();
                let mut request = client.post(url).body(output.to_string());

                for (key, value) in headers {
                    request = request.header(key, value);
                }

                request
                    .send()
                    .await
                    .map_err(|e| MantaError::Http(e))?;

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

        debug!(
            "Job run logged: {} - {:?}",
            entry.job_id, entry.status
        );

        // TODO: Persist to JSONL file
        let _ = entry;

        Ok(())
    }

    /// Load jobs from store
    async fn load_jobs(&mut self, path: &PathBuf) -> Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            MantaError::Internal(format!("Failed to read jobs file: {}", e))
        })?;

        let jobs: Vec<AdvancedCronJob> =
            serde_json::from_str(&content).map_err(|e| {
                MantaError::Internal(format!("Failed to parse jobs: {}", e))
            })?;

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
    async fn save_jobs(
        jobs: &Arc<RwLock<HashMap<String, AdvancedCronJob>>>,
        path: &PathBuf,
    ) -> Result<()> {
        let jobs_lock = jobs.read().await;
        let jobs_vec: Vec<&AdvancedCronJob> = jobs_lock.values().collect();

        let json = serde_json::to_string_pretty(&jobs_vec).map_err(|e| {
            MantaError::Internal(format!("Failed to serialize jobs: {}", e))
        })?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        tokio::fs::write(path, json).await.map_err(|e| {
            MantaError::Internal(format!("Failed to write jobs file: {}", e))
        })?;

        Ok(())
    }

    /// Public API methods

    /// Add a job
    pub async fn add_job(&self, job: AdvancedCronJob) -> Result<()> {
        self.command_tx
            .send(CronCommand::Add(job))
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to add job: {}", e)))
    }

    /// Remove a job
    pub async fn remove_job(&self, job_id: &str) -> Result<()> {
        self.command_tx
            .send(CronCommand::Remove(job_id.to_string()))
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to remove job: {}", e)))
    }

    /// Enable/disable a job
    pub async fn set_job_enabled(&self, job_id: &str, enabled: bool) -> Result<()> {
        self.command_tx
            .send(CronCommand::SetEnabled(job_id.to_string(), enabled))
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to set job state: {}", e)))
    }

    /// Trigger a job immediately
    pub async fn trigger_job(&self, job_id: &str) -> Result<()> {
        self.command_tx
            .send(CronCommand::Trigger(job_id.to_string()))
            .await
            .map_err(|e| MantaError::Internal(format!("Failed to trigger job: {}", e)))
    }

    /// List all jobs
    pub async fn list_jobs(&self) -> Vec<AdvancedCronJob> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = self.command_tx.send(CronCommand::ListJobs(tx)).await;
        rx.await.unwrap_or_default()
    }

    /// Get a specific job
    pub async fn get_job(&self, job_id: &str) -> Option<AdvancedCronJob> {
        let (tx, rx) = tokio::sync::oneshot::channel();
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
        if let Some(handle) = self.timer_handle.take() {
            handle.abort();
        }
        Ok(())
    }
}

impl Default for AdvancedCronScheduler {
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

        let schedule = Schedule::Every {
            interval,
            anchor: None,
        };

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
}
