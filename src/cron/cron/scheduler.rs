//! `CronScheduler` construction, the global-timer loop, command handling,
//! and the public job-management API.

use super::*;

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
            schedule_change_tx: None,
            rearm_notify: Arc::new(tokio::sync::Notify::new()),
            inflight: Arc::new(TokioMutex::new(Vec::new())),
        };
        (scheduler, command_rx)
    }

    /// Attach a schedule-change sender (§4.3).
    ///
    /// The receiver is typically drained by the gateway and forwarded to a
    /// platform wake bridge. Not wired on desktop (`None`), where emitting a
    /// snapshot is a no-op.
    pub fn set_schedule_change_tx(&mut self, tx: mpsc::Sender<ScheduleChangeSnapshot>) {
        self.schedule_change_tx = Some(tx);
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
        let schedule_change_tx = self.schedule_change_tx.clone();
        let inflight = Arc::clone(&self.inflight);
        let rearm_notify = Arc::clone(&self.rearm_notify);

        // Emit the initial schedule snapshot after load_jobs so a background
        // platform wake bridge (WorkManager, §4.3) re-establishes its alarm
        // set from a fresh scheduler start / app relaunch.
        Self::emit_schedule_snapshot(&jobs, &schedule_change_tx).await;

        // Spawn command handler
        let cmd_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    cmd = command_rx.recv() => {
                        if let Some(cmd) = cmd {
                            Self::handle_command(&jobs, &agent, &store_path, &announce_tx, &heartbeat_wake_tx, &inflight, &rearm_notify, &schedule_change_tx, cmd).await;
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
    pub(super) async fn run_due_jobs(
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

    /// Handle scheduler commands
    #[allow(clippy::too_many_arguments)]
    async fn handle_command(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        agent: &Arc<RwLock<Option<Arc<Agent>>>>,
        store_path: &Option<PathBuf>,
        announce_tx: &Option<mpsc::Sender<AnnounceDelivery>>,
        heartbeat_wake_tx: &Option<mpsc::Sender<crate::heartbeat::WakeRequest>>,
        inflight: &Arc<TokioMutex<Vec<(String, AbortHandle)>>>,
        rearm_notify: &Arc<tokio::sync::Notify>,
        schedule_change_tx: &Option<mpsc::Sender<ScheduleChangeSnapshot>>,
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

                // Notify the timer so it recalculates with the new job
                // included. This is done inside handle_command — not at the
                // call site — so there is no TOCTOU window where the timer
                // wakes up before the job is inserted into the map.
                rearm_notify.notify_one();

                // Sync the changed schedule to any platform wake bridge.
                Self::emit_schedule_snapshot(jobs, schedule_change_tx).await;
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

                // Rearm so the timer no longer waits for the removed job's
                // next_run_at — preventing a phantom wake once the old
                // sleep expires.
                rearm_notify.notify_one();

                // Sync the changed schedule to any platform wake bridge.
                Self::emit_schedule_snapshot(jobs, schedule_change_tx).await;
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

                // Always rearm — when enabling (timer may have been waiting
                // for this job's next run) and when disabling (timer may be
                // sleeping toward this job, so we recalibrate without it).
                rearm_notify.notify_one();

                // Sync the changed schedule to any platform wake bridge.
                Self::emit_schedule_snapshot(jobs, schedule_change_tx).await;
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

    /// Emit a snapshot of `(job_id, next_run_at_ms)` for every enabled job
    /// (§4.3). If no schedule-change receiver is wired (desktop), this is a
    /// no-op. Uses `try_send` so the command actor never blocks on a slow
    /// consumer; a `Full` result just drops the update — the next schedule
    /// change re-sends a fresh snapshot, and the platform bridge syncs the
    /// full set each time.
    async fn emit_schedule_snapshot(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        schedule_change_tx: &Option<mpsc::Sender<ScheduleChangeSnapshot>>,
    ) {
        let Some(tx) = schedule_change_tx else {
            return;
        };
        let snapshot = {
            let jobs_lock = jobs.read().await;
            jobs_lock
                .iter()
                .filter(|(_, job)| job.enabled)
                .map(|(id, job)| {
                    let next_ms = job.state.next_run_at.map(|t| t.timestamp_millis());
                    (id.clone(), next_ms)
                })
                .collect::<Vec<_>>()
        };
        match tx.try_send(snapshot) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                // Consumer busy; the next schedule change re-syncs.
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                debug!("schedule_change_tx receiver dropped");
            }
        }
    }

    /// Push an abort handle into the in-flight tracker, reaping any
    /// already-finished entries first so the list stays bounded by the
    /// count of currently-running tasks rather than total spawn count.
    /// Tagged with `job_id` so `abort_job` can target a specific job's
    /// outstanding executions.
    pub(super) async fn push_inflight(
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
    async fn abort_job(inflight: &Arc<TokioMutex<Vec<(String, AbortHandle)>>>, job_id: &str) {
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

    /// Build structured delivery payload for a completed job execution.
    ///
    /// Returns a JSON object with `status: "ok"` on success or
    /// `status: "error"` on failure, along with the job identity and
    /// execution timestamp. This is a pure function — it does not mutate
    /// any job state — so callers can invoke it inside a write-lock
    /// section without worrying about ordering.
    pub(super) fn build_delivery_payload(
        result: &Result<String>,
        job_name: &str,
        job_id: &str,
        completed_at: DateTime<Utc>,
    ) -> serde_json::Value {
        match result {
            Ok(output) => serde_json::json!({
                "job_name": job_name,
                "job_id": job_id,
                "status": "ok",
                "output": output.trim(),
                "run_at": completed_at.to_rfc3339(),
            }),
            Err(e) => serde_json::json!({
                "job_name": job_name,
                "job_id": job_id,
                "status": "error",
                "error": format!("{}", e),
                "run_at": completed_at.to_rfc3339(),
            }),
        }
    }

    /// Add a job
    pub async fn add_job(&self, job: CronJob) -> Result<()> {
        self.command_tx
            .send(CronCommand::Add(job))
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to add job: {}", e)))
    }

    /// Remove a job
    pub async fn remove_job(&self, job_id: &str) -> Result<()> {
        self.command_tx
            .send(CronCommand::Remove(job_id.to_string()))
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to remove job: {}", e)))
    }

    /// Enable/disable a job
    pub async fn set_job_enabled(&self, job_id: &str, enabled: bool) -> Result<()> {
        self.command_tx
            .send(CronCommand::SetEnabled(job_id.to_string(), enabled))
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to set job state: {}", e)))
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
