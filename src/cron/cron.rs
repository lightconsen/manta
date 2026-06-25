//! Advanced Cron Scheduler for Syscity
//!
//! Production-grade scheduler supporting AI agent execution, multi-channel
//! delivery, and enterprise reliability features.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use cron::Schedule as CronSchedule;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex as TokioMutex, RwLock};
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{debug, error, info, warn};

use crate::agent::Agent;
use crate::channels::IncomingMessage;
use crate::error::{Result, SyscityError};

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
#[derive(Default)]
pub enum SessionTarget {
    /// Run in main session (has conversation context)
    Main,
    /// Run in isolated session (clean state: cron:{job_id})
    #[default]
    Isolated,
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
    ///
    /// **Note on the `timezone` field**: cron expressions are currently
    /// always evaluated in UTC. The `timezone` field is preserved for
    /// forward compatibility but has no effect today; a warning is
    /// emitted at job registration time when a non-`None` value is
    /// supplied so the limitation is visible. Schedules that need local
    /// time must convert externally for now.
    Cron {
        expression: String,
        /// **Currently unsupported** — always evaluated as UTC. Setting
        /// this field logs a one-time warning at job registration but
        /// otherwise has no effect on schedule resolution.
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
                let interval_secs = interval.as_secs();
                if interval_secs == 0 {
                    warn!(
                        "Schedule::Every interval is zero — job will never run \
                         (interval must be >= 1 second)"
                    );
                    return None;
                }

                let anchor = anchor.unwrap_or(from);

                if from < anchor {
                    Some(anchor)
                } else {
                    let elapsed = from.signed_duration_since(anchor);
                    let interval_i64 = interval_secs as i64;
                    let periods = elapsed
                        .num_seconds()
                        .checked_div(interval_i64)?
                        .checked_add(1)?;
                    let offset_secs = periods.checked_mul(interval_i64)?;
                    Some(anchor + ChronoDuration::seconds(offset_secs))
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
                let normalized = if expression.split_whitespace().count() == 5 {
                    format!("0 {}", expression.trim())
                } else {
                    expression.clone()
                };

                let schedule = match CronSchedule::from_str(&normalized) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(
                            "invalid cron expression {:?}: {} — job will never run",
                            expression, e
                        );
                        return None;
                    }
                };

                // Get next occurrence
                let next = schedule.upcoming(Utc).next()?;

                // Add stagger if configured. `% 0` panics, so we treat
                // `Some(0)` the same as `None` — no jitter.
                match stagger_ms {
                    Some(stagger) if *stagger > 0 => {
                        let jitter = rand::random::<u64>() % stagger;
                        Some(next + ChronoDuration::milliseconds(jitter as i64))
                    }
                    _ => Some(next),
                }
            }
        }
    }

    /// Check if this is a one-shot schedule that should be deleted after
    /// execution
    pub fn is_one_shot(&self) -> bool {
        matches!(self, Schedule::At { .. })
    }

    /// Emit one-shot warnings for partially-supported schedule fields.
    ///
    /// Called once per job at registration / load time (Add command and
    /// `load_jobs`) so that contract violations don't disappear into a
    /// silent loop. Currently surfaces:
    ///
    /// - `Schedule::Cron { timezone: Some(_) }` — field ignored, always
    ///   evaluated in UTC.
    pub fn warn_unsupported_fields(&self, job_name: &str) {
        if let Schedule::Cron { timezone: Some(tz), .. } = self {
            warn!(
                "cron job '{}' specifies timezone={:?} which is currently \
                 unsupported — schedule will be evaluated in UTC",
                job_name, tz
            );
        }
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

/// Wake mode for cron-triggered heartbeat
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WakeMode {
    /// No wake — run cron job normally
    #[default]
    None,
    /// Wake the heartbeat runner immediately after job execution
    HeartbeatWake,
}

impl WakeMode {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
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
    /// Wake mode — when set to `HeartbeatWake`, sends a wake request to the
    /// heartbeat runner after the job completes.
    #[serde(default, skip_serializing_if = "WakeMode::is_none")]
    pub wake_mode: WakeMode,
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
            wake_mode: WakeMode::None,
        }
    }

    /// Set the wake mode for heartbeat integration
    pub fn with_wake_mode(mut self, wake_mode: WakeMode) -> Self {
        self.wake_mode = wake_mode;
        self
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

/// Maximum timer delay - wake at least once per minute to check for schedule
/// changes
const MAX_TIMER_DELAY_MS: u64 = 60_000;
/// Minimum delay between timer fires to prevent tight loops
const MIN_REFIRE_GAP_MS: u64 = 2_000;
/// Hard upper bound on a single shell command's execution time.
/// A hung process otherwise keeps `running_at_ms` set forever, making
/// the job permanently un-runnable until the scheduler restarts.
const SHELL_EXEC_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Hard upper bound on a single agent task's execution time.
const AGENT_EXEC_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Hard upper bound on the bytes captured from a shell job's stdout or
/// stderr. Excess output is discarded (the child eventually blocks on
/// its pipe and is reaped by `kill_on_drop` when the outer timeout
/// fires). 1 MiB is enough to capture any reasonable error message
/// without letting a misconfigured `cat /dev/urandom` OOM the gateway.
const MAX_SHELL_OUTPUT_BYTES: usize = 1024 * 1024;

/// Commands for the scheduler
#[allow(clippy::large_enum_variant)]
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

/// Advanced cron scheduler with single global timer
///
/// # Concurrency model
///
/// Writes are serialised through the `command_tx` actor channel: only
/// `handle_command` mutates the `jobs` map. Reads, however, bypass the
/// actor and acquire the `RwLock` directly (see `calculate_next_wake_ms`
/// and `run_due_jobs`). This is intentional — the timer needs lock-step
/// access to the schedule without round-tripping through the command
/// queue. Do **not** add reader paths that assume actor serialisation;
/// the only ordering guarantee is "writes are linearised, reads see a
/// consistent snapshot taken under a read lock".
pub struct CronScheduler {
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    command_tx: mpsc::Sender<CronCommand>,
    /// Broadcast so both the command-handler and timer tasks can subscribe.
    shutdown_tx: Option<broadcast::Sender<()>>,
    /// JoinHandles of the inner background tasks (command handler, timer).
    /// Tracked so `shutdown()` can await/abort them; the gateway-level
    /// `TaskRegistry` only tracks the outer `start()` wrapper task.
    inner_handles: Vec<JoinHandle<()>>,
    /// Shared agent reference — wrapping in `Arc<RwLock<…>>` lets
    /// `set_agent()` update the running scheduler's agent without
    /// restarting any background tasks.
    agent: Arc<RwLock<Option<Arc<Agent>>>>,
    store_path: Option<PathBuf>,
    /// Optional sender for Announce-mode delivery events.
    announce_tx: Option<mpsc::Sender<AnnounceDelivery>>,
    /// Optional sender for heartbeat wake requests.
    /// When a cron job has `wake_mode: HeartbeatWake`, a wake request is sent
    /// here.
    heartbeat_wake_tx: Option<mpsc::Sender<crate::heartbeat::WakeRequest>>,
    /// Notify the timer to re-calculate next wake time
    rearm_notify: Arc<tokio::sync::Notify>,
    /// Abort handles of in-flight job tasks, each tagged with the job
    /// id that spawned it.
    ///
    /// Two paths register here:
    ///
    /// - `CronCommand::Trigger` spawns `execute_job` as a detached task
    ///   so the command actor is not blocked for the full job duration.
    /// - `execute_job` spawns the agent inner future (so timeout aborts
    ///   propagate to the next `.await` inside the agent).
    ///
    /// Each push first reaps finished handles via `push_inflight`, so the
    /// vector stays bounded by the count of currently in-flight tasks
    /// rather than growing once per job execution over the scheduler's
    /// lifetime. `CronCommand::Remove` calls `abort_job` to cancel any
    /// in-flight execution that belongs to the job being deleted, so the
    /// removed job stops burning CPU writing back to a record that has
    /// just been erased. On `shutdown()` we abort every remaining entry
    /// so nothing survives the scheduler.
    inflight: Arc<TokioMutex<Vec<(String, AbortHandle)>>>,
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
            inner_handles: Vec::new(),
            agent: Arc::new(RwLock::new(None)),
            store_path: None,
            announce_tx: None,
            heartbeat_wake_tx: None,
            rearm_notify: Arc::new(tokio::sync::Notify::new()),
            inflight: Arc::new(TokioMutex::new(Vec::new())),
        };
        (scheduler, command_rx)
    }

    /// Attach an announce delivery sender.
    ///
    /// When a cron job uses `DeliveryMode::Announce`, the scheduler sends an
    /// [`AnnounceDelivery`] event on this channel. The caller is responsible
    /// for receiving the events and routing them to the correct messaging
    /// back-end.
    pub fn set_announce_tx(&mut self, tx: mpsc::Sender<AnnounceDelivery>) {
        self.announce_tx = Some(tx);
    }

    /// Attach a heartbeat wake sender.
    ///
    /// When a cron job has `wake_mode: HeartbeatWake`, a wake request is sent
    /// to this channel after the job completes.
    pub fn set_heartbeat_wake_tx(&mut self, tx: mpsc::Sender<crate::heartbeat::WakeRequest>) {
        self.heartbeat_wake_tx = Some(tx);
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
        // Broadcast so both the command-handler and timer tasks can subscribe.
        // Capacity 1 is enough — we only ever send a single `()` shutdown.
        let (shutdown_tx, mut cmd_shutdown_rx) = broadcast::channel::<()>(1);
        let mut timer_shutdown_rx = shutdown_tx.subscribe();
        self.shutdown_tx = Some(shutdown_tx);

        // Load jobs from store if configured
        let store_path = self.store_path.clone();
        if let Some(ref path) = store_path {
            self.load_jobs(path)
                .await
                .map_err(|e| warn!("failed to load persisted cron jobs at startup: {e}"))
                .ok();
        }

        let jobs = Arc::clone(&self.jobs);
        let agent = Arc::clone(&self.agent);
        let store_path = self.store_path.clone();
        let announce_tx = self.announce_tx.clone();
        let heartbeat_wake_tx = self.heartbeat_wake_tx.clone();
        let inflight = Arc::clone(&self.inflight);

        // Spawn command handler
        let cmd_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    cmd = command_rx.recv() => {
                        if let Some(cmd) = cmd {
                            Self::handle_command(&jobs, &agent, &store_path, &announce_tx, &heartbeat_wake_tx, &inflight, cmd).await;
                        }
                    }
                    _ = cmd_shutdown_rx.recv() => {
                        info!("Cron scheduler command handler shutting down");
                        break;
                    }
                }
            }
        });
        self.inner_handles.push(cmd_handle);

        // Spawn single global timer task
        let jobs_for_timer = Arc::clone(&self.jobs);
        let agent_for_timer = Arc::clone(&self.agent);
        let store_path_for_timer = self.store_path.clone();
        let announce_tx_for_timer = self.announce_tx.clone();
        let heartbeat_wake_tx_for_timer = self.heartbeat_wake_tx.clone();
        let rearm_notify = Arc::clone(&self.rearm_notify);
        let inflight_for_timer = Arc::clone(&self.inflight);

        let timer_handle = tokio::spawn(async move {
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

                // Wait for timer OR rearm notification OR shutdown
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
                    _ = timer_shutdown_rx.recv() => {
                        info!("Cron scheduler timer shutting down");
                        break;
                    }
                }

                // Check if already running (prevent overlapping ticks)
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
                let heartbeat_wake_tx = heartbeat_wake_tx_for_timer.clone();
                let inflight = Arc::clone(&inflight_for_timer);

                // Run jobs (result unused). Wrap in select! against shutdown
                // so a long-running batch cannot block graceful exit.
                let mut shutdown_during_jobs = false;
                tokio::select! {
                    _ = Self::run_due_jobs(
                        &jobs,
                        &agent,
                        &store_path,
                        &announce_tx,
                        &heartbeat_wake_tx,
                        &inflight,
                    ) => {
                        // batch completed normally
                    }
                    _ = timer_shutdown_rx.recv() => {
                        warn!(
                            "Cron scheduler shutting down with jobs in flight; \
                             abandoning batch. running_at_ms will be cleared on next start."
                        );
                        shutdown_during_jobs = true;
                    }
                }

                // Always mark as not running and continue (re-arm happens at loop start)
                *running.write().await = false;

                if shutdown_during_jobs {
                    info!("Cron scheduler timer shutting down (mid-batch)");
                    break;
                }

                // The loop continues and re-arms automatically
            }
        });
        self.inner_handles.push(timer_handle);

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
    ///
    /// Due jobs are dispatched concurrently via [`tokio::task::JoinSet`].
    /// A single slow job (within its execution timeout) no longer blocks
    /// peers due in the same tick. The function still awaits the entire
    /// set so the outer timer's shutdown `select!` continues to bound the
    /// batch — abandoning the awaited JoinSet at shutdown drops all
    /// inner JoinHandles, which (combined with `kill_on_drop` on shell
    /// children and the explicit abort path on agent tasks inside
    /// `execute_job`) releases resources promptly.
    async fn run_due_jobs(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        agent: &Arc<RwLock<Option<Arc<Agent>>>>,
        store_path: &Option<PathBuf>,
        announce_tx: &Option<mpsc::Sender<AnnounceDelivery>>,
        heartbeat_wake_tx: &Option<mpsc::Sender<crate::heartbeat::WakeRequest>>,
        inflight: &Arc<TokioMutex<Vec<(String, AbortHandle)>>>,
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

        info!("Running {} due cron jobs concurrently", due_job_ids.len());

        let mut set = tokio::task::JoinSet::new();
        for job_id in due_job_ids {
            let jobs = Arc::clone(jobs);
            let agent = Arc::clone(agent);
            let store_path = store_path.clone();
            let announce_tx = announce_tx.clone();
            let heartbeat_wake_tx = heartbeat_wake_tx.clone();
            let inflight = Arc::clone(inflight);
            set.spawn(async move {
                Self::execute_job(
                    &jobs,
                    &job_id,
                    &agent,
                    &store_path,
                    &announce_tx,
                    &heartbeat_wake_tx,
                    &inflight,
                    false,
                )
                .await;
            });
        }

        while let Some(join_res) = set.join_next().await {
            if let Err(e) = join_res {
                if !e.is_cancelled() {
                    warn!("cron job task join error: {}", e);
                }
            }
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
        heartbeat_wake_tx: &Option<mpsc::Sender<crate::heartbeat::WakeRequest>>,
        inflight: &Arc<TokioMutex<Vec<(String, AbortHandle)>>>,
        cmd: CronCommand,
    ) {
        match cmd {
            CronCommand::Add(mut job) => {
                info!("Adding job: {} ({})", job.name, job.id);

                // Surface unsupported schedule fields exactly once per job
                // registration so contract violations don't disappear into
                // a silent loop.
                job.schedule.warn_unsupported_fields(&job.name);

                // Calculate initial next run
                if job.state.next_run_at.is_none() {
                    job.update_next_run(Utc::now());
                }

                jobs.write().await.insert(job.id.clone(), job);

                // Persist
                if let Some(ref path) = store_path {
                    Self::save_jobs(jobs, path)
                        .await
                        .unwrap_or_else(|e| warn!("Failed to persist cron jobs (Add): {}", e));
                }
            }
            CronCommand::Remove(id) => {
                info!("Removing job: {}", id);
                // Cancel any in-flight execution belonging to this job before
                // dropping it from the map — otherwise the orphan task would
                // keep running and write back to a record that no longer
                // exists.
                Self::abort_job(inflight, &id).await;
                jobs.write().await.remove(&id);

                if let Some(ref path) = store_path {
                    Self::save_jobs(jobs, path)
                        .await
                        .unwrap_or_else(|e| warn!("Failed to persist cron jobs (Remove): {}", e));
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
                    Self::save_jobs(jobs, path).await.unwrap_or_else(|e| {
                        warn!("Failed to persist cron jobs (SetEnabled): {}", e)
                    });
                }
            }
            CronCommand::Trigger(id) => {
                info!("Triggering job: {}", id);
                // Spawn the job execution detached so the command actor is
                // not blocked for the full job duration. Register the
                // abort handle so `shutdown()` can cancel it.
                let jobs_c = Arc::clone(jobs);
                let agent_c = Arc::clone(agent);
                let store_path_c = store_path.clone();
                let announce_tx_c = announce_tx.clone();
                let heartbeat_wake_tx_c = heartbeat_wake_tx.clone();
                let inflight_c = Arc::clone(inflight);
                let id_for_spawn = id.clone();
                let handle = tokio::spawn(async move {
                    Self::execute_job(
                        &jobs_c,
                        &id_for_spawn,
                        &agent_c,
                        &store_path_c,
                        &announce_tx_c,
                        &heartbeat_wake_tx_c,
                        &inflight_c,
                        true,
                    )
                    .await;
                });
                Self::push_inflight(inflight, id, handle.abort_handle()).await;
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

    /// Push an abort handle into the in-flight tracker, reaping any
    /// already-finished entries first so the list stays bounded by the
    /// count of currently-running tasks rather than total spawn count.
    /// Tagged with `job_id` so `abort_job` can target a specific job's
    /// outstanding executions.
    async fn push_inflight(
        inflight: &Arc<TokioMutex<Vec<(String, AbortHandle)>>>,
        job_id: String,
        handle: AbortHandle,
    ) {
        let mut guard = inflight.lock().await;
        guard.retain(|(_, h)| !h.is_finished());
        guard.push((job_id, handle));
    }

    /// Abort any in-flight executions associated with `job_id` and drop
    /// their handles from the tracker. Called when a job is removed so
    /// the orphan task does not continue running and writing back to a
    /// state record that no longer exists.
    async fn abort_job(
        inflight: &Arc<TokioMutex<Vec<(String, AbortHandle)>>>,
        job_id: &str,
    ) {
        let mut guard = inflight.lock().await;
        guard.retain(|(id, h)| {
            if id == job_id {
                h.abort();
                false
            } else {
                !h.is_finished()
            }
        });
    }

    /// Execute a job
    ///
    /// When `force` is true, the job runs regardless of `should_run` /
    /// `next_run_at`. Used by manual trigger (`Trigger` command).
    /// Timer-driven execution passes `false`.
    #[allow(clippy::too_many_arguments)]
    async fn execute_job(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        job_id: &str,
        agent: &Arc<RwLock<Option<Arc<Agent>>>>,
        store_path: &Option<PathBuf>,
        announce_tx: &Option<mpsc::Sender<AnnounceDelivery>>,
        heartbeat_wake_tx: &Option<mpsc::Sender<crate::heartbeat::WakeRequest>>,
        inflight: &Arc<TokioMutex<Vec<(String, AbortHandle)>>>,
        force: bool,
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

            // Check if should run (skip when forced)
            let now = Utc::now();
            if !force && !job.should_run(now) {
                return;
            }

            // Even when `force=true`, refuse to double-execute a job that is
            // already running. Without this guard, two concurrent
            // `execute_job` calls would both clear and rewrite
            // `running_at_ms`, run the underlying work twice, and corrupt
            // `run_count` / `consecutive_errors`.
            if job.state.running_at_ms.is_some() {
                warn!(
                    "Trigger ignored: cron job '{}' is already running (running_at_ms={:?})",
                    job.name, job.state.running_at_ms
                );
                return;
            }

            // Mark as running
            job.state.running_at_ms = Some(now.timestamp_millis());
            job.clone()
        };

        info!("Executing job: {}", job.name);
        let run_id = uuid::Uuid::new_v4().to_string();
        let started_at = Utc::now();

        // Execute based on target type. Each path is wrapped in a hard
        // timeout — without it a hung process or stalled agent keeps
        // `running_at_ms` set forever, making the job permanently
        // un-runnable. The shell path relies on `kill_on_drop(true)` inside
        // `execute_shell` so the child process is reaped when the future
        // is cancelled. The agent path spawns into a separate task and
        // uses `abort_handle()` so that on timeout we propagate
        // cancellation to the next `.await` point inside the agent —
        // dropping the future alone would not.
        let result = match &job.target {
            ExecutionTarget::Shell { command } => {
                match tokio::time::timeout(SHELL_EXEC_TIMEOUT, Self::execute_shell(command)).await {
                    Ok(r) => r,
                    Err(_) => {
                        // The future is dropped here; `kill_on_drop` on
                        // the child Child handle inside execute_shell
                        // sends SIGKILL to the underlying `sh` process.
                        Err(SyscityError::Internal(format!(
                            "Shell command timed out after {:?}",
                            SHELL_EXEC_TIMEOUT
                        )))
                    }
                }
            }
            ExecutionTarget::Agent { prompt, agent_id, .. } => {
                let agent_clone = {
                    let agent_guard = agent.read().await;
                    agent_guard.as_ref().map(Arc::clone)
                };
                if let Some(agent_ref) = agent_clone {
                    let job_clone = job.clone();
                    let prompt_owned = prompt.clone();
                    let agent_id_owned = agent_id.clone();
                    let handle = tokio::spawn(async move {
                        Self::execute_agent(
                            &agent_ref,
                            &job_clone,
                            &prompt_owned,
                            agent_id_owned.as_deref(),
                        )
                        .await
                    });
                    let abort_handle = handle.abort_handle();
                    // Register the agent inner spawn so `shutdown()` can
                    // abort it. Without this, the inner task would survive
                    // the scheduler.
                    Self::push_inflight(inflight, job_id.to_string(), abort_handle.clone()).await;
                    match tokio::time::timeout(AGENT_EXEC_TIMEOUT, handle).await {
                        Ok(Ok(r)) => r,
                        Ok(Err(e)) => {
                            Err(SyscityError::Internal(format!("Agent task join error: {}", e)))
                        }
                        Err(_) => {
                            // Explicitly abort so cancellation actually
                            // reaches the agent at its next `.await`,
                            // rather than letting the task continue
                            // detached after we give up waiting.
                            abort_handle.abort();
                            Err(SyscityError::Internal(format!(
                                "Agent task timed out after {:?}",
                                AGENT_EXEC_TIMEOUT
                            )))
                        }
                    }
                } else {
                    Err(SyscityError::Internal("No agent configured for cron job".to_string()))
                }
            }
        };

        let completed_at = Utc::now();

        // Update job state. We update everything that requires the write
        // lock here, but defer the actual delivery (which can `.await` for
        // up to ~90s on webhook retries) until after the lock is released.
        // Holding the write lock across delivery would serialise every
        // other scheduler operation (Add/Remove/Trigger/timer) behind one
        // slow HTTP call.
        let delivery_intent = {
            let mut jobs_lock = jobs.write().await;
            if let Some(j) = jobs_lock.get_mut(job_id) {
                j.state.running_at_ms = None;
                j.state.last_run_at = Some(completed_at);
                j.state.run_count += 1;

                // Build structured delivery payload for both success and error
                let delivery_payload = match &result {
                    Ok(output) => {
                        j.state.last_error = None;
                        j.state.consecutive_errors = 0;
                        info!("Job '{}' completed successfully", j.name);

                        serde_json::json!({
                            "job_name": j.name,
                            "job_id": j.id,
                            "status": "ok",
                            "output": output.trim(),
                            "run_at": completed_at.to_rfc3339(),
                        })
                    }
                    Err(e) => {
                        let error_msg = format!("{}", e);
                        j.state.last_error = Some(error_msg.clone());
                        j.state.consecutive_errors += 1;
                        error!("Job '{}' failed: {}", j.name, error_msg);

                        serde_json::json!({
                            "job_name": j.name,
                            "job_id": j.id,
                            "status": "error",
                            "error": error_msg,
                            "run_at": completed_at.to_rfc3339(),
                        })
                    }
                };

                // Capture what delivery needs so we can `.await` it outside
                // the lock. Cloning is cheap relative to the alternative of
                // freezing the whole scheduler for the duration of an HTTP
                // round-trip.
                let intent = if matches!(j.delivery, DeliveryMode::None) {
                    None
                } else {
                    let message = serde_json::to_string(&delivery_payload)
                        .unwrap_or_else(|_| delivery_payload.to_string());
                    Some((j.delivery.clone(), message, j.name.clone()))
                };

                // Update next run (or schedule retry on error).
                //
                // Note: one-shot (`Schedule::At`) jobs are removed below, so
                // there is no point computing a retry slot for them — the
                // job ceases to exist after this block.
                match &result {
                    Ok(_) => j.update_next_run(completed_at),
                    Err(_) if j.schedule.is_one_shot() => {
                        // One-shot: no retry, no next_run — the job is about
                        // to be removed.
                    }
                    Err(_) => {
                        if j.state.consecutive_errors <= j.retry.max_retries {
                            // delay_for_attempt is 0-indexed: attempt 0 → 30s,
                            // attempt 1 → 60s, etc. consecutive_errors is the
                            // count of failures so far (>=1 here), so subtract
                            // one to start the first retry at the 30s tier.
                            let attempt = j.state.consecutive_errors.saturating_sub(1);
                            let delay = j.retry.delay_for_attempt(attempt);
                            let retry_at = completed_at
                                + chrono::Duration::from_std(delay)
                                    .unwrap_or_else(|_| chrono::Duration::seconds(60));
                            warn!("Scheduling retry for job '{}' at {:?}", j.name, retry_at);
                            j.state.next_run_at = Some(retry_at);
                        } else {
                            // Max retries exhausted — disable the job so it
                            // stops burning resources on a doomed task.
                            // Operator must re-enable explicitly after
                            // fixing the underlying cause.
                            error!(
                                "Job '{}' disabled after {} consecutive failures (max_retries={})",
                                j.name, j.state.consecutive_errors, j.retry.max_retries
                            );
                            j.enabled = false;
                            j.state.next_run_at = None;
                        }
                    }
                }

                // Remove one-shot jobs after execution
                if j.schedule.is_one_shot() {
                    info!("Removing one-shot job: {}", j.name);
                    jobs_lock.remove(job_id);
                }

                intent
            } else {
                None
            }
        };

        // Deliver result OUTSIDE the write lock. The delivery channel can
        // be a webhook with up to ~30s × 3 retries; under the old
        // structure that would have frozen the whole scheduler. With the
        // lock released, concurrent Add/Remove/Trigger and the timer
        // continue to make progress.
        //
        // Capture the outcome so the run log can record whether delivery
        // actually succeeded — not just whether execution succeeded.
        let delivery_status = if let Some((delivery, message, job_name)) = delivery_intent {
            match Self::deliver_result(&delivery, &message, announce_tx).await {
                Ok(()) => Some(DeliveryStatus::Delivered),
                Err(e) => {
                    warn!("Delivery failed for job '{}': {}", job_name, e);
                    Some(DeliveryStatus::Failed(e.to_string()))
                }
            }
        } else {
            // DeliveryMode::None — there is no delivery to report on.
            None
        };

        // Persist
        if let Some(ref path) = store_path {
            if let Err(e) = Self::save_jobs(jobs, path).await {
                warn!("Failed to persist cron jobs after run: {e}");
            }
        }

        // Log the run
        if let Err(e) = Self::log_run(
            job_id,
            &run_id,
            started_at,
            completed_at,
            result,
            delivery_status,
            store_path,
        )
        .await
        {
            warn!("Failed to log cron run: {e}");
        }

        // Send heartbeat wake if configured and job succeeded
        if matches!(job.wake_mode, WakeMode::HeartbeatWake) {
            if let Some(ref tx) = heartbeat_wake_tx {
                let agent_id = match &job.target {
                    ExecutionTarget::Agent { agent_id, .. } => agent_id.clone().unwrap_or_default(),
                    _ => String::from("*"),
                };
                info!(
                    "Cron job '{}' completed with heartbeat wake — waking agent {}",
                    job.name, agent_id
                );
                // Use `try_send` rather than `send().await`: this hop is
                // strictly best-effort, and an `.await` here would stall
                // the job-completion path indefinitely if the heartbeat
                // channel is saturated. Both `Full` and `Closed` are
                // logged but otherwise ignored — the next cron tick
                // will get another chance.
                if let Err(e) = tx.try_send(crate::heartbeat::WakeRequest {
                    agent_id,
                    priority: crate::heartbeat::WakePriority::Action,
                    prompt: None,
                }) {
                    warn!("Cron job '{}' could not send heartbeat wake request: {}", job.name, e);
                }
            }
        }
    }

    /// Execute shell command
    ///
    /// Spawn is used (rather than `output()`) for two correctness reasons:
    ///
    /// 1. `kill_on_drop(true)` guarantees that if our caller's
    ///    `tokio::time::timeout` fires, the underlying `sh` process is
    ///    SIGKILL'd as the `Child` is dropped. Without this, the timeout
    ///    only stops *us* waiting; the child keeps running, holds FDs,
    ///    and may spawn even more work.
    /// 2. stdout/stderr are drained on background tasks but only the
    ///    first `MAX_SHELL_OUTPUT_BYTES` are retained. Bytes past the cap
    ///    are read and discarded so the child can keep writing without
    ///    blocking on a full pipe buffer (which would otherwise cause
    ///    SIGPIPE / non-zero exit on downstream tools in a pipeline, or
    ///    hang the child indefinitely). The retained buffer cannot OOM
    ///    the gateway because it is capped.
    async fn execute_shell(command: &str) -> Result<String> {
        use tokio::io::AsyncReadExt;

        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| SyscityError::Internal(format!("Failed to execute shell: {}", e)))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SyscityError::Internal("shell child has no stdout pipe".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SyscityError::Internal("shell child has no stderr pipe".to_string()))?;

        /// Drain `reader` to EOF, returning at most `MAX_SHELL_OUTPUT_BYTES`
        /// of the head of the stream. Bytes past the cap are discarded but
        /// still read from the pipe so the child does not block on a full
        /// OS pipe buffer.
        async fn drain_capped<R: AsyncReadExt + Unpin>(
            mut reader: R,
            stream_name: &str,
        ) -> Vec<u8> {
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if buf.len() < MAX_SHELL_OUTPUT_BYTES {
                            let remaining = MAX_SHELL_OUTPUT_BYTES - buf.len();
                            let take = n.min(remaining);
                            buf.extend_from_slice(&chunk[..take]);
                        }
                        // Anything past the cap is intentionally discarded
                        // but the pipe has already been drained.
                    }
                    Err(e) => {
                        warn!("error reading cron shell {}: {}", stream_name, e);
                        break;
                    }
                }
            }
            buf
        }

        let stdout_task = tokio::spawn(drain_capped(stdout, "stdout"));
        let stderr_task = tokio::spawn(drain_capped(stderr, "stderr"));

        let status = child
            .wait()
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to wait on shell: {}", e)))?;

        let stdout_bytes = stdout_task.await.unwrap_or_default();
        let stderr_bytes = stderr_task.await.unwrap_or_default();

        if stdout_bytes.len() >= MAX_SHELL_OUTPUT_BYTES {
            warn!(
                "cron shell stdout truncated at {} bytes (job may be producing unbounded output)",
                MAX_SHELL_OUTPUT_BYTES
            );
        }

        if status.success() {
            Ok(String::from_utf8_lossy(&stdout_bytes).to_string())
        } else {
            let stderr_str = String::from_utf8_lossy(&stderr_bytes);
            Err(SyscityError::Internal(format!("Shell error: {}", stderr_str)))
        }
    }

    /// Execute via agent
    ///
    /// `agent_id` is attached to the outgoing message metadata so
    /// downstream routing can dispatch the work to a specific sub-agent.
    /// `None` lets the routing layer pick the default agent.
    async fn execute_agent(
        agent: &Arc<Agent>,
        job: &CronJob,
        prompt: &str,
        agent_id: Option<&str>,
    ) -> Result<String> {
        let session_id = match job.session {
            SessionTarget::Main => "cron:main".to_string(),
            SessionTarget::Isolated => format!("cron:{}", job.id),
        };

        let mut metadata = crate::channels::MessageMetadata::new()
            .with_extra("job_id", job.id.clone())
            .with_extra("job_name", job.name.clone());
        if let Some(id) = agent_id {
            metadata = metadata.with_extra("agent_id", id.to_string());
        }

        let message = IncomingMessage::new("system", &session_id, prompt)
            .with_provenance(crate::channels::InputProvenance::InternalSystem {
                source: "cron".to_string(),
            })
            .with_metadata(metadata);

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
                    .map_err(|_| SyscityError::Internal("Announce channel closed".to_string()))?;
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

                let client = reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .map_err(|e| {
                        SyscityError::Internal(format!("Failed to create HTTP client: {}", e))
                    })?;

                const MAX_ATTEMPTS: u32 = 3;
                let mut last_error = String::new();

                for attempt in 1..=MAX_ATTEMPTS {
                    let mut request = client.post(url).body(output.to_string());

                    for (key, value) in headers {
                        request = request.header(key, value);
                    }

                    match request.send().await {
                        Ok(response) => {
                            if response.status().is_success() {
                                debug!("Webhook delivery succeeded on attempt {}", attempt);
                                return Ok(());
                            }
                            let status = response.status();
                            last_error = format!("HTTP {}", status);
                            warn!(
                                "Webhook delivery failed on attempt {}/{}: status {}",
                                attempt, MAX_ATTEMPTS, status
                            );
                        }
                        Err(e) => {
                            last_error = e.to_string();
                            warn!(
                                "Webhook delivery failed on attempt {}/{}: {}",
                                attempt, MAX_ATTEMPTS, e
                            );
                        }
                    }

                    if attempt < MAX_ATTEMPTS {
                        let delay = Duration::from_secs(1 << (attempt - 1));
                        debug!(
                            "Retrying webhook delivery in {:?} (attempt {})",
                            delay,
                            attempt + 1
                        );
                        tokio::time::sleep(delay).await;
                    }
                }

                Err(SyscityError::Internal(format!(
                    "Webhook delivery failed after {} attempts: {}",
                    MAX_ATTEMPTS, last_error
                )))
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
        delivery_status: Option<DeliveryStatus>,
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
                delivery_status,
            },
            Err(e) => RunLogEntry {
                run_id: run_id.to_string(),
                job_id: job_id.to_string(),
                started_at,
                completed_at: Some(completed_at),
                status: RunStatus::Error,
                output: None,
                error: Some(format!("{}", e)),
                // When execution itself failed, the delivery field still
                // reflects whatever happened with the error-payload
                // delivery (or `None` for `DeliveryMode::None`).
                delivery_status,
            },
        };

        debug!("Job run logged: {} - {:?}", entry.job_id, entry.status);

        // Persist to JSONL file if store_path is configured
        if let Some(ref path) = store_path {
            let log_path = path.with_extension("runs.jsonl");
            let line = serde_json::to_string(&entry).map_err(|e| {
                SyscityError::Internal(format!("Failed to serialize run log: {}", e))
            })?;
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .await
                .map_err(|e| SyscityError::Internal(format!("Failed to open run log: {}", e)))?;
            use tokio::io::AsyncWriteExt;
            file.write_all(line.as_bytes())
                .await
                .map_err(|e| SyscityError::Internal(format!("Failed to write run log: {}", e)))?;
            file.write_all(b"\n")
                .await
                .map_err(|e| SyscityError::Internal(format!("Failed to write run log: {}", e)))?;
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
            .map_err(|e| SyscityError::Internal(format!("Failed to read jobs file: {}", e)))?;

        let jobs: Vec<CronJob> = serde_json::from_str(&content)
            .map_err(|e| SyscityError::Internal(format!("Failed to parse jobs: {}", e)))?;

        let mut jobs_lock = self.jobs.write().await;
        for job in jobs {
            // Clear stale running markers (crash recovery)
            let mut job = job;
            if job.state.running_at_ms.is_some() {
                job.state.running_at_ms = None;
                job.state.last_error = Some("Recovered from crash".to_string());
            }

            // Surface any unsupported schedule fields on reload so the
            // operator sees the limitation in the startup log rather than
            // discovering it silently months later.
            job.schedule.warn_unsupported_fields(&job.name);

            jobs_lock.insert(job.id.clone(), job);
        }

        info!("Loaded {} jobs from store", jobs_lock.len());
        Ok(())
    }

    /// Save jobs to store
    ///
    /// Writes are atomic: serialise to `path.tmp`, then `rename` over `path`.
    /// A mid-write crash leaves the old file intact rather than truncating
    /// the jobs store and losing every persisted job on next start.
    async fn save_jobs(jobs: &Arc<RwLock<HashMap<String, CronJob>>>, path: &PathBuf) -> Result<()> {
        let jobs_lock = jobs.read().await;
        let jobs_vec: Vec<&CronJob> = jobs_lock.values().collect();

        let json = serde_json::to_string_pretty(&jobs_vec)
            .map_err(|e| SyscityError::Internal(format!("Failed to serialize jobs: {}", e)))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                warn!("failed to create cron jobs store directory {parent:?}: {e}");
            }
        }

        // Atomic write: tmp + rename. The rename is atomic on POSIX and
        // Windows (since NTFS w/ MoveFileExW). On crash we keep the old
        // file rather than corrupting the live one.
        let mut tmp_path = path.clone();
        let mut tmp_name = path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        tmp_name.push(".tmp");
        tmp_path.set_file_name(tmp_name);

        tokio::fs::write(&tmp_path, json)
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to write jobs tmp file: {}", e)))?;

        tokio::fs::rename(&tmp_path, path).await.map_err(|e| {
            // Best-effort cleanup of the stale tmp file; ignore the result.
            let tmp_for_cleanup = tmp_path.clone();
            tokio::spawn(async move {
                if let Err(e) = tokio::fs::remove_file(&tmp_for_cleanup).await {
                    warn!("failed to clean up cron jobs tmp file {tmp_for_cleanup:?}: {e}");
                }
            });
            SyscityError::Internal(format!("Failed to rename jobs tmp file into place: {}", e))
        })?;

        Ok(())
    }

    /// Add a job
    pub async fn add_job(&self, job: CronJob) -> Result<()> {
        self.command_tx
            .send(CronCommand::Add(job))
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to add job: {}", e)))?;
        // Trigger rearm to pick up new job
        self.trigger_rearm();
        Ok(())
    }

    /// Remove a job
    pub async fn remove_job(&self, job_id: &str) -> Result<()> {
        self.command_tx
            .send(CronCommand::Remove(job_id.to_string()))
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to remove job: {}", e)))?;
        self.trigger_rearm();
        Ok(())
    }

    /// Enable/disable a job
    pub async fn set_job_enabled(&self, job_id: &str, enabled: bool) -> Result<()> {
        self.command_tx
            .send(CronCommand::SetEnabled(job_id.to_string(), enabled))
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to set job state: {}", e)))?;
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
            .map_err(|e| SyscityError::Internal(format!("Failed to trigger job: {}", e)))
    }

    /// List all jobs
    ///
    /// Returns an empty `Vec` if the scheduler has been shut down or the
    /// command channel is closed; the failure is logged at `warn!` so an
    /// empty list under that condition is not silent.
    pub async fn list_jobs(&self) -> Vec<CronJob> {
        let (tx, rx) = oneshot::channel();
        if let Err(e) = self.command_tx.send(CronCommand::ListJobs(tx)).await {
            warn!("list_jobs: command channel closed: {e}");
            return Vec::new();
        }
        match rx.await {
            Ok(list) => list,
            Err(e) => {
                warn!("list_jobs: response channel closed before reply: {e}");
                Vec::new()
            }
        }
    }

    /// Get a specific job
    ///
    /// Returns `None` both for an absent job and for a closed command/response
    /// channel; the channel-failure cases are logged at `warn!`.
    pub async fn get_job(&self, job_id: &str) -> Option<CronJob> {
        let (tx, rx) = oneshot::channel();
        if let Err(e) = self
            .command_tx
            .send(CronCommand::GetJob(job_id.to_string(), tx))
            .await
        {
            warn!("get_job({job_id}): command channel closed: {e}");
            return None;
        }
        match rx.await {
            Ok(opt) => opt,
            Err(e) => {
                warn!("get_job({job_id}): response channel closed before reply: {e}");
                None
            }
        }
    }

    /// Shutdown the scheduler
    pub async fn shutdown(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            // Broadcast send fails only when no receivers remain — by then
            // the inner tasks have already exited, so ignore that case.
            let _ = tx.send(());
        }

        // Abort any in-flight detached job tasks (triggered jobs, agent
        // inner spawns). Without this they would continue running after
        // the scheduler is gone.
        {
            let mut inflight = self.inflight.lock().await;
            for (_id, abort) in inflight.drain(..) {
                abort.abort();
            }
        }

        // Drain inner JoinHandles and wait briefly for graceful exit; abort
        // any that don't finish in time. This guarantees no orphaned tasks
        // survive a shutdown() call.
        let handles = std::mem::take(&mut self.inner_handles);
        for handle in handles {
            let abort = handle.abort_handle();
            match tokio::time::timeout(Duration::from_secs(5), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if !e.is_cancelled() {
                        warn!("Cron inner task join error: {}", e);
                    }
                }
                Err(_) => {
                    warn!("Cron inner task did not exit within 5s; aborting");
                    abort.abort();
                }
            }
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
    fn test_schedule_next_run_every_zero_interval_returns_none() {
        // Regression: previously panicked with divide-by-zero
        let schedule = Schedule::Every {
            interval: Duration::ZERO,
            anchor: None,
        };
        assert_eq!(schedule.next_run(Utc::now()), None);
    }

    #[test]
    fn test_schedule_next_run_every_subsecond_interval_returns_none() {
        // Sub-second intervals truncate to 0 seconds — also rejected
        let schedule = Schedule::Every {
            interval: Duration::from_millis(500),
            anchor: None,
        };
        assert_eq!(schedule.next_run(Utc::now()), None);
    }

    #[test]
    fn test_schedule_next_run_cron_invalid_returns_none() {
        // Regression: invalid cron expression must not panic and must not
        // silently succeed — it returns None so the scheduler skips the job.
        let schedule = Schedule::Cron {
            expression: "not a cron expression".to_string(),
            timezone: None,
            stagger_ms: None,
        };
        assert_eq!(schedule.next_run(Utc::now()), None);
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

    #[tokio::test]
    async fn test_say_hi_every_2_min_job() {
        let cron_dir = crate::dirs::cron_dir();
        let store_path = cron_dir.join("test-say-hi.json");
        let runs_path = cron_dir.join("test-say-hi.runs.jsonl");

        // Clean up any previous test artifacts
        let _ = tokio::fs::remove_file(&store_path).await;
        let _ = tokio::fs::remove_file(&runs_path).await;

        // Create and start scheduler
        let (mut scheduler, command_rx) = CronScheduler::new();
        scheduler.store_path = Some(store_path.clone());
        scheduler.start(command_rx).await.unwrap();

        // Create the "say-hi-every-2-min" job
        let job = CronJob::new(
            "say-hi-001",
            "say-hi-every-2-min",
            Schedule::Cron {
                expression: "*/2 * * * *".to_string(),
                timezone: None,
                stagger_ms: None,
            },
            ExecutionTarget::Shell {
                command: "echo 'hi from cron'".to_string(),
            },
        )
        .with_delivery(DeliveryMode::None);

        scheduler.add_job(job).await.unwrap();

        // Verify job is in scheduler
        let jobs = scheduler.list_jobs().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "say-hi-every-2-min");
        assert_eq!(jobs[0].id, "say-hi-001");
        assert!(
            matches!(jobs[0].target, ExecutionTarget::Shell { ref command } if command == "echo 'hi from cron'")
        );

        // Verify persistence file written
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(store_path.exists(), "jobs store file should exist");

        let store_content = tokio::fs::read_to_string(&store_path).await.unwrap();
        assert!(store_content.contains("say-hi-every-2-min"));
        assert!(store_content.contains("*/2 * * * *"));

        // Trigger immediate execution
        scheduler.trigger_job("say-hi-001").await.unwrap();

        // Wait for execution to complete
        tokio::time::sleep(Duration::from_millis(800)).await;

        // Verify run log
        assert!(runs_path.exists(), "runs log file should exist after execution");
        let runs_content = tokio::fs::read_to_string(&runs_path).await.unwrap();
        assert!(!runs_content.is_empty(), "runs log should contain at least one entry");
        assert!(runs_content.contains("say-hi-001"), "run log should reference job id");

        // Verify job state updated
        let jobs_after = scheduler.list_jobs().await;
        assert_eq!(jobs_after[0].state.run_count, 1);
        assert!(jobs_after[0].state.last_run_at.is_some());
        assert!(jobs_after[0].state.last_error.is_none());

        // Cleanup
        let _ = tokio::fs::remove_file(&store_path).await;
        let _ = tokio::fs::remove_file(&runs_path).await;

        scheduler.shutdown().await.unwrap();
    }

    #[test]
    fn test_wake_mode_default() {
        assert_eq!(WakeMode::default(), WakeMode::None);
    }

    #[test]
    fn test_wake_mode_is_none() {
        assert!(WakeMode::None.is_none());
        assert!(!WakeMode::HeartbeatWake.is_none());
    }

    #[test]
    fn test_wake_mode_serialize_none_skipped() {
        let job = CronJob::new(
            "test",
            "Test",
            Schedule::At { timestamp: Utc::now() },
            ExecutionTarget::shell("echo"),
        );
        let json = serde_json::to_string(&job).unwrap();
        // WakeMode::None should be skipped due to skip_serializing_if
        assert!(!json.contains("wake_mode"));
    }

    #[test]
    fn test_wake_mode_serialize_heartbeat_wake() {
        let job = CronJob::new(
            "test",
            "Test",
            Schedule::At { timestamp: Utc::now() },
            ExecutionTarget::shell("echo"),
        )
        .with_wake_mode(WakeMode::HeartbeatWake);
        let json = serde_json::to_string(&job).unwrap();
        assert!(json.contains("\"wake_mode\":\"heartbeat_wake\""));
    }

    #[test]
    fn test_wake_mode_roundtrip() {
        let job = CronJob::new(
            "test",
            "Test",
            Schedule::At { timestamp: Utc::now() },
            ExecutionTarget::agent("check status"),
        )
        .with_wake_mode(WakeMode::HeartbeatWake);
        let json = serde_json::to_string(&job).unwrap();
        let deserialized: CronJob = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.wake_mode, WakeMode::HeartbeatWake);
        assert_eq!(deserialized.id, job.id);
        // Also verify the agent target is preserved
        assert!(matches!(deserialized.target, ExecutionTarget::Agent { .. }));
    }

    // ── Negative tests: persistence error handling ──────────────────────────

    #[tokio::test]
    async fn test_save_jobs_fails_on_readonly_path() {
        let jobs: Arc<RwLock<HashMap<String, CronJob>>> = Arc::new(RwLock::new(HashMap::new()));
        let dir = tempfile::tempdir().expect("tempdir");
        let subdir = dir.path().join("locked");
        std::fs::create_dir(&subdir).unwrap();
        let path = subdir.join("cron_jobs.json");

        // Make the *parent directory* read-only so save_jobs cannot create the
        // tmp file (atomic write writes to <path>.tmp first, then renames).
        // We use the directory rather than just the file because rename(2)
        // ignores the destination file's permissions and only checks the
        // directory's write bit.
        let mut perms = std::fs::metadata(&subdir).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&subdir, perms).unwrap();

        let result = CronScheduler::save_jobs(&jobs, &path).await;

        // Restore write perms so the tempdir can be cleaned up.
        let mut perms = std::fs::metadata(&subdir).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(&subdir, perms);

        assert!(result.is_err(), "save_jobs should fail when the target directory is read-only");
    }

    /// M3: save_jobs writes via a tmp file + rename, and on success leaves no
    /// `.tmp` file behind in the target directory.
    #[tokio::test]
    async fn test_save_jobs_is_atomic_and_leaves_no_tmp_file() {
        let mut map: HashMap<String, CronJob> = HashMap::new();
        let future = Utc::now() + ChronoDuration::hours(1);
        let job = CronJob::new(
            "atomic-job",
            "Atomic Test",
            Schedule::At { timestamp: future },
            ExecutionTarget::shell("echo hi"),
        );
        map.insert(job.id.clone(), job);
        let jobs: Arc<RwLock<HashMap<String, CronJob>>> = Arc::new(RwLock::new(map));

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cron_jobs.json");
        let tmp_path = dir.path().join("cron_jobs.json.tmp");

        CronScheduler::save_jobs(&jobs, &path).await.unwrap();

        assert!(path.exists(), "final jobs file should exist after save");
        assert!(!tmp_path.exists(), "tmp file should be renamed away, not left behind");

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("atomic-job"), "saved file should contain the job id");
    }

    #[tokio::test]
    async fn test_log_run_fails_on_readonly_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store_path = dir.path().join("cron_jobs.json");
        // log_run derives the log path as <store_path>.runs.jsonl
        let log_path = dir.path().join("cron_jobs.runs.jsonl");

        // Create the runs.jsonl file and make it read-only so the
        // OpenOptions::append inside log_run fails.
        tokio::fs::write(&log_path, b"").await.unwrap();
        let mut perms = std::fs::metadata(&log_path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&log_path, perms).unwrap();

        let now = Utc::now();
        let result = CronScheduler::log_run(
            "test-job",
            "test-run-id",
            now,
            now + ChronoDuration::seconds(1),
            Ok("output".to_string()),
            Some(DeliveryStatus::Delivered),
            &Some(store_path),
        )
        .await;
        assert!(result.is_err(), "log_run should fail when runs.jsonl is read-only");
    }

    #[tokio::test]
    async fn test_execute_job_persistence_failure_does_not_panic() {
        let jobs: Arc<RwLock<HashMap<String, CronJob>>> = Arc::new(RwLock::new(HashMap::new()));
        let agent: Arc<RwLock<Option<Arc<Agent>>>> = Arc::new(RwLock::new(None));

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cron_jobs.json");

        // Make the store path read-only so save_jobs + log_run fail.
        tokio::fs::write(&path, b"").await.unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();

        let job = CronJob::new(
            "test-job",
            "Test Job",
            Schedule::Every {
                interval: Duration::from_secs(3600),
                anchor: None,
            },
            ExecutionTarget::shell("echo hello"),
        );
        jobs.write().await.insert("test-job".to_string(), job);

        // Execute with force=true; persistence will fail but must not panic
        let inflight = Arc::new(TokioMutex::new(Vec::new()));
        CronScheduler::execute_job(
            &jobs,
            "test-job",
            &agent,
            &Some(path),
            &None, // announce_tx
            &None, // heartbeat_wake_tx
            &inflight,
            true, // force
        )
        .await;

        // Job state must still be updated despite persistence failure
        let updated = jobs.read().await.get("test-job").cloned().unwrap();
        assert!(
            updated.state.running_at_ms.is_none(),
            "running_at_ms should be cleared after execution"
        );
        assert_eq!(updated.state.run_count, 1, "run_count should be incremented after execution");
        assert!(updated.state.last_error.is_none(), "no error for successful shell execution");
    }

    // ── Regression tests for H1 / H2 / M1 / M2 / M3 / M4 fixes ────────────

    /// M1: a force-trigger must NOT double-execute a job that is already
    /// running. Previously, `force=true` bypassed the running_at_ms check
    /// entirely and would overwrite `running_at_ms` with a fresh
    /// timestamp, leaving two concurrent execution paths.
    #[tokio::test]
    async fn test_force_trigger_respects_running_at_ms() {
        let mut map: HashMap<String, CronJob> = HashMap::new();
        let mut job = CronJob::new(
            "running-job",
            "Running",
            Schedule::At {
                timestamp: Utc::now() + ChronoDuration::hours(1),
            },
            ExecutionTarget::shell("echo hi"),
        );
        // Simulate the job already being in flight.
        job.state.running_at_ms = Some(Utc::now().timestamp_millis());
        map.insert(job.id.clone(), job);
        let jobs: Arc<RwLock<HashMap<String, CronJob>>> = Arc::new(RwLock::new(map));
        let agent: Arc<RwLock<Option<Arc<Agent>>>> = Arc::new(RwLock::new(None));

        let before = jobs.read().await.get("running-job").unwrap().state.clone();

        // Force-trigger should be refused; run_count must not advance and
        // running_at_ms must not change.
        let inflight = Arc::new(TokioMutex::new(Vec::new()));
        CronScheduler::execute_job(
            &jobs,
            "running-job",
            &agent,
            &None,
            &None,
            &None,
            &inflight,
            true, // force
        )
        .await;

        let after = jobs.read().await.get("running-job").unwrap().state.clone();
        assert_eq!(before.running_at_ms, after.running_at_ms, "running_at_ms preserved");
        assert_eq!(
            before.run_count, after.run_count,
            "run_count must not advance on refused trigger"
        );
    }

    /// M4: shell stdout is bounded — a job that emits unbounded output
    /// must not load gigabytes into memory; the captured output should be
    /// at most MAX_SHELL_OUTPUT_BYTES.
    #[tokio::test]
    async fn test_execute_shell_caps_unbounded_stdout() {
        // `yes` produces an infinite stream of "y\n". With our cap the
        // process either blocks on its pipe (and is killed when this test
        // ends and the parent process drops the child) or completes
        // quickly when the OS pipe buffer fills. Either way, the output
        // we observe must not exceed the cap.
        //
        // Wrap in a timeout so a buggy implementation fails fast rather
        // than hanging the test suite.
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            CronScheduler::execute_shell("yes | head -c 5242880"), // 5 MiB
        )
        .await
        .expect("execute_shell should complete within timeout");

        let out = result.expect("shell command should succeed");
        assert!(
            out.len() <= MAX_SHELL_OUTPUT_BYTES,
            "captured stdout {} bytes must not exceed cap {}",
            out.len(),
            MAX_SHELL_OUTPUT_BYTES
        );
    }

    /// H2: the shell timeout path must reap the child process via
    /// `kill_on_drop`. We confirm correctness indirectly by verifying
    /// that timeouts actually return — without `kill_on_drop`, the child
    /// would keep the pipe FDs alive and `wait_with_output`'s drop
    /// could leak. A simpler invariant we can check: a long-running
    /// command, wrapped in a short timeout, must come back as a timeout
    /// error in well under SHELL_EXEC_TIMEOUT.
    #[tokio::test]
    async fn test_shell_timeout_returns_promptly() {
        // 100ms timeout against a 30s sleep — must return as Err quickly,
        // i.e. the timeout actually unblocks the future.
        let start = tokio::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            CronScheduler::execute_shell("sleep 30"),
        )
        .await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "expected timeout, got {:?}", result);
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout should return promptly (within ~2s) but took {:?}",
            elapsed
        );
    }

    /// M2: a Cron schedule with a `timezone` field must emit a warning
    /// via `warn_unsupported_fields` so the limitation is visible. The
    /// warning is a side-effect we can't capture from inside the test,
    /// but we can at least verify the method does not panic for
    /// supported and unsupported variants.
    #[test]
    fn test_warn_unsupported_fields_does_not_panic() {
        let s1 = Schedule::Cron {
            expression: "0 * * * *".to_string(),
            timezone: Some("America/Los_Angeles".to_string()),
            stagger_ms: None,
        };
        s1.warn_unsupported_fields("test-job");

        let s2 = Schedule::Cron {
            expression: "0 * * * *".to_string(),
            timezone: None,
            stagger_ms: None,
        };
        s2.warn_unsupported_fields("test-job");

        let s3 = Schedule::At { timestamp: Utc::now() };
        s3.warn_unsupported_fields("test-job");
    }

    /// M3: run_due_jobs dispatches due jobs concurrently. We check this
    /// by registering N shell jobs that each sleep 500ms and verifying
    /// the whole batch finishes in under N * 500ms wall clock. Serial
    /// execution would take N * 500ms; concurrent execution finishes in
    /// roughly 500ms regardless of N.
    #[tokio::test]
    async fn test_run_due_jobs_executes_concurrently() {
        let mut map: HashMap<String, CronJob> = HashMap::new();
        let now = Utc::now();
        for i in 0..4 {
            let mut job = CronJob::new(
                format!("concurrent-{}", i),
                format!("Concurrent {}", i),
                Schedule::At {
                    timestamp: now + ChronoDuration::hours(1),
                },
                ExecutionTarget::shell("sleep 0.5"),
            );
            // Make should_run() true: due now, not yet running.
            job.state.next_run_at = Some(now - ChronoDuration::seconds(1));
            job.state.running_at_ms = None;
            map.insert(job.id.clone(), job);
        }
        let jobs: Arc<RwLock<HashMap<String, CronJob>>> = Arc::new(RwLock::new(map));
        let agent: Arc<RwLock<Option<Arc<Agent>>>> = Arc::new(RwLock::new(None));

        let start = tokio::time::Instant::now();
        let inflight = Arc::new(TokioMutex::new(Vec::new()));
        CronScheduler::run_due_jobs(&jobs, &agent, &None, &None, &None, &inflight).await;
        let elapsed = start.elapsed();

        // Serial: ~2.0s (4 × 500ms). Concurrent: ~0.5s. Use 1.5s as a
        // safe upper bound that fails serial but tolerates CI jitter.
        assert!(
            elapsed < Duration::from_millis(1500),
            "4 due jobs each sleeping 500ms must run concurrently; \
             actual elapsed: {:?}",
            elapsed
        );

        // Sanity: all jobs ran (one-shot, so they are removed by
        // execute_job after running).
        let remaining = jobs.read().await;
        assert!(
            remaining.is_empty(),
            "all one-shot jobs should have been removed after execution; \
             remaining: {:?}",
            remaining.keys().collect::<Vec<_>>()
        );
    }

    /// `% 0` panics. Schedule construction with `stagger_ms = Some(0)`
    /// must be treated as "no stagger" and not panic at next-run-time.
    #[test]
    fn test_schedule_cron_stagger_zero_does_not_panic() {
        let sched = Schedule::Cron {
            expression: "0 * * * * *".to_string(), // every minute on :00
            timezone: None,
            stagger_ms: Some(0),
        };
        // Without the guard, this would panic with "attempt to calculate
        // the remainder with a divisor of zero" inside `next_run`.
        let next = sched.next_run(Utc::now());
        assert!(next.is_some(), "stagger_ms=0 must still produce a next run");
    }

    /// Once `consecutive_errors` exceeds `max_retries`, the scheduler
    /// disables the job rather than continuing to retry forever.
    #[tokio::test]
    async fn test_job_disabled_after_max_retries_exhausted() {
        let mut map: HashMap<String, CronJob> = HashMap::new();
        let mut job = CronJob::new(
            "doomed",
            "Doomed",
            // Use a recurring schedule so the failure path retries
            // instead of being a one-shot.
            Schedule::Every {
                interval: Duration::from_secs(60),
                anchor: None,
            },
            ExecutionTarget::shell("exit 1"),
        );
        job.retry.max_retries = 2;
        // Simulate two failures already on the books — the next failed
        // run lands at consecutive_errors = 3, which exceeds max_retries.
        job.state.consecutive_errors = 2;
        map.insert(job.id.clone(), job);
        let jobs: Arc<RwLock<HashMap<String, CronJob>>> = Arc::new(RwLock::new(map));
        let agent: Arc<RwLock<Option<Arc<Agent>>>> = Arc::new(RwLock::new(None));
        let inflight = Arc::new(TokioMutex::new(Vec::new()));

        CronScheduler::execute_job(
            &jobs, "doomed", &agent, &None, &None, &None, &inflight, true, // force
        )
        .await;

        let after = jobs.read().await.get("doomed").cloned().unwrap();
        assert!(!after.enabled, "job must be disabled after exhausting retries");
        assert!(after.state.next_run_at.is_none(), "next_run_at cleared on disable");
        assert!(after.state.consecutive_errors > after.retry.max_retries);
    }

    /// `log_run` must record whatever delivery outcome was passed in —
    /// not synthesise a value from the execution result. This is the
    /// regression for the previous hard-coded `Delivered`/`Execution
    /// error` mapping that ignored real delivery failures.
    #[tokio::test]
    async fn test_log_run_records_supplied_delivery_status() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("cron.json");

        let now = Utc::now();
        CronScheduler::log_run(
            "j",
            "r",
            now,
            now + ChronoDuration::seconds(1),
            Ok("output".to_string()),
            Some(DeliveryStatus::Failed("webhook 500".to_string())),
            &Some(store.clone()),
        )
        .await
        .expect("log_run");

        let log_path = store.with_extension("runs.jsonl");
        let line = tokio::fs::read_to_string(&log_path)
            .await
            .expect("read log");
        let entry: RunLogEntry = serde_json::from_str(line.trim()).expect("parse");
        assert_eq!(entry.status, RunStatus::Ok);
        assert_eq!(
            entry.delivery_status,
            Some(DeliveryStatus::Failed("webhook 500".to_string())),
            "delivery_status must reflect the caller-supplied outcome, \
             not be synthesised from execution result"
        );
    }

    /// `push_inflight` must reap finished abort handles instead of letting
    /// the in-flight vector grow once per spawn forever. Without pruning,
    /// a long-running scheduler accumulates one entry per job execution
    /// across its lifetime.
    #[tokio::test]
    async fn test_push_inflight_reaps_finished_handles() {
        let inflight = Arc::new(TokioMutex::new(Vec::new()));

        // Push 5 handles whose tasks have already completed.
        for _ in 0..5 {
            let handle = tokio::spawn(async {});
            // Let it run to completion.
            handle.await.expect("task ran");
            // Spawn a no-op specifically to get an already-finished
            // abort_handle. The simpler path: spawn, await, then
            // capture the AbortHandle from a fresh handle that we
            // know is done.
            let done = tokio::spawn(async {});
            done.await.expect("ran");
            let h = tokio::spawn(async {});
            let abort = h.abort_handle();
            h.await.expect("ran");
            // `abort` now refers to a finished task.
            CronScheduler::push_inflight(&inflight, "test-job".to_string(), abort).await;
        }

        // The vector contains 5 finished handles plus possibly the last
        // one we just pushed. Before the next push, all 5 should be
        // reaped, and then the new handle is appended — never more than
        // one finished tail + one live entry.
        let len_before_live = inflight.lock().await.len();
        assert!(
            len_before_live <= 5,
            "list should not have grown unboundedly: {}",
            len_before_live
        );

        // Now push a live, never-completing task and verify reap drops
        // the finished tail.
        let live = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        let live_abort = live.abort_handle();
        CronScheduler::push_inflight(&inflight, "live-job".to_string(), live_abort).await;

        // After this push, all the previously-finished handles should
        // have been reaped, leaving only the live one.
        let final_len = inflight.lock().await.len();
        assert_eq!(
            final_len, 1,
            "push_inflight should retain only the unfinished live handle, got {}",
            final_len
        );

        // Cleanup
        live.abort();
    }
}
