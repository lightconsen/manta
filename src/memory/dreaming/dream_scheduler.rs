//! Cron-driven dreaming scheduler.

use super::*;

/// A scheduled dreaming service that runs dreams via cron.
#[derive(Clone)]
pub struct DreamScheduler {
    engine: Arc<DreamEngine>,
    /// Handle to the background scheduling task (for cancellation)
    shutdown_tx: Option<tokio::sync::mpsc::Sender<()>>,
    /// Cancel signal sender for stopping in-progress dream cycles
    cancel_tx: Option<tokio::sync::watch::Sender<bool>>,
}

impl DreamScheduler {
    /// Create a new scheduler around the given engine.
    pub fn new(engine: Arc<DreamEngine>) -> Self {
        Self {
            engine,
            shutdown_tx: None,
            cancel_tx: None,
        }
    }

    /// Run a one-off dream cycle immediately.
    pub async fn run_now(
        &self,
        store: &dyn MemoryStore,
        tier_index: &TierIndex,
        include_rem: bool,
        llm_callback: Option<&LlmCallback>,
    ) -> crate::Result<Vec<DreamResult>> {
        // Create a cancel signal that never cancels
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        self.engine
            .run_full_cycle(store, tier_index, include_rem, llm_callback, cancel_rx)
            .await
    }

    /// Get the shared metrics.
    pub fn metrics(&self) -> Arc<DreamMetrics> {
        self.engine.metrics()
    }

    /// Start the background cron scheduler.
    ///
    /// Spawns a tokio task that sleeps until the next cron tick, runs the
    /// appropriate dream phase(s), then re-arms.  Call [`stop()`] to shut down.
    /// Returns the spawned task handle so the caller can register it with a
    /// [`TaskRegistry`] and await graceful shutdown.
    pub fn start(
        &mut self,
        store: Arc<dyn MemoryStore>,
        tier_index: Arc<TierIndex>,
    ) -> tokio::task::JoinHandle<()> {
        if !self.engine.config.enabled {
            info!("Dreaming is disabled; scheduler not started");
            return tokio::spawn(async {});
        }

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel(1);
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        self.cancel_tx = Some(cancel_tx);

        let engine = Arc::clone(&self.engine);
        let frequency = self.engine.config.frequency.clone();

        tokio::spawn(async move {
            let schedule = match CronSchedule::from_str(&frequency) {
                Ok(s) => s,
                Err(e) => {
                    warn!("Invalid dream cron expression '{}': {}", frequency, e);
                    return;
                }
            };

            loop {
                // Calculate next execution time
                let next = match schedule.upcoming(Utc).next() {
                    Some(dt) => dt,
                    None => {
                        warn!("No upcoming dream times for cron '{}'", frequency);
                        break;
                    }
                };

                let now = Utc::now();
                let delay_ms = if next > now {
                    (next - now).num_milliseconds().max(0) as u64
                } else {
                    0
                };

                let sleep_deadline = TokioInstant::now() + Duration::from_millis(delay_ms);
                info!("Next dream scheduled at {} (in {} ms)", next, delay_ms);

                tokio::select! {
                    _ = sleep_until(sleep_deadline) => {
                        info!("Running scheduled dream cycle");
                        let include_rem = engine.config.budget == DreamBudget::Expensive;
                        let cancel = cancel_rx.clone();
                        match engine.run_full_cycle(store.as_ref(), tier_index.as_ref(), include_rem, None, cancel).await {
                            Ok(results) => {
                                for r in &results {
                                    info!("Dream result: {}", r.summary);
                                }
                            }
                            Err(e) => {
                                warn!("Scheduled dream cycle failed: {}", e);
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Dream scheduler shutting down");
                        break;
                    }
                }
            }
        })
    }

    /// Stop the background scheduler and cancel any in-progress dream cycles.
    pub async fn stop(&mut self) {
        // First send cancellation signal to any in-progress dream
        if let Some(tx) = self.cancel_tx.take() {
            if tx.send(true).is_err() {
                debug!("Failed to send dream cancellation signal (receiver already dropped)");
            }
        }
        // Then send shutdown signal to the scheduler loop
        if let Some(tx) = self.shutdown_tx.take() {
            if let Err(e) = tx.send(()).await {
                debug!("Failed to send dream scheduler shutdown signal: {:?}", e);
            }
        }
    }

    /// Returns true if the scheduler background task is running.
    pub fn is_running(&self) -> bool {
        self.shutdown_tx.is_some()
    }
}
