//! Subagent Registry
//!
//! Tracks in-process subagent spawning with depth/concurrency limits and
//! provides lifecycle management (spawn, wait, kill).  All agents run in the
//! same Tokio runtime — no external processes.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::error::MantaError;

/// Current lifecycle state of a subagent run.
#[derive(Debug, Clone)]
pub enum SubagentStatus {
    /// Still executing.
    Running,
    /// Finished with output.
    Completed(String),
    /// Terminated abnormally.
    Failed(String),
    /// Killed by the caller.
    Killed,
}

/// Metadata and result for one subagent invocation.
#[derive(Debug, Clone)]
pub struct SubagentRun {
    /// Unique run identifier.
    pub run_id: String,
    /// Session that spawned this run.
    pub parent_session: String,
    /// Session allocated for the child.
    pub child_session: String,
    /// Name of the agent handling the task.
    pub target_agent: String,
    /// Current status.
    pub status: SubagentStatus,
    /// Nesting depth (root = 1).
    pub spawn_depth: u32,
    /// Wall-clock start time.
    pub started_at: Instant,
    /// Wall-clock completion time (if done).
    pub completed_at: Option<Instant>,
}

impl SubagentRun {
    /// Elapsed time since the run started.
    pub fn elapsed(&self) -> Duration {
        self.completed_at
            .unwrap_or_else(Instant::now)
            .duration_since(self.started_at)
    }
}

/// Metrics for subagent runs tracked by the registry.
#[derive(Debug, Default, Clone)]
pub struct SubagentMetrics {
    /// Total runs ever spawned.
    pub total_spawned: u64,
    /// Runs that completed successfully.
    pub total_completed: u64,
    /// Runs that failed.
    pub total_failed: u64,
    /// Runs that were killed.
    pub total_killed: u64,
}

// ── Persistence types ─────────────────────────────────────────────────────────

/// Terminal outcome for a persisted run record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RunOutcome {
    /// The run completed successfully with this output.
    Completed(String),
    /// The run failed with this error message.
    Failed(String),
}

/// A serializable snapshot of a completed subagent run, used for crash recovery.
///
/// Uses `SystemTime` (wall clock) instead of `Instant` so it can be stored
/// to disk and loaded on restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Unique run identifier.
    pub run_id: String,
    /// Session that spawned this run.
    pub parent_session: String,
    /// Session allocated for the child.
    pub child_session: String,
    /// Agent that handled the task.
    pub target_agent: String,
    /// Nesting depth at the time of spawn.
    pub spawn_depth: u32,
    /// Wall-clock time the run started.
    pub started_at: SystemTime,
    /// Wall-clock time the run finished.
    pub completed_at: SystemTime,
    /// Terminal outcome.
    pub outcome: RunOutcome,
}

/// Registry for in-process subagent lifecycle management.
///
/// # Example
///
/// ```rust,no_run
/// # use std::sync::Arc;
/// # use manta::agent::subagent_registry::SubagentRegistry;
/// # use std::time::Duration;
/// # async fn example() -> manta::error::Result<()> {
/// let registry = Arc::new(SubagentRegistry::new(3, 10));
///
/// // Spawn returns a run_id; the actual execution is your responsibility
/// // (wire in your Arc<Agent> via the callback argument).
/// let run_id = registry
///     .spawn("session-1", "code-reviewer", "review this PR", {
///         let registry = Arc::clone(&registry);
///         move |run_id, _task| {
///             let registry = Arc::clone(&registry);
///             Box::pin(async move {
///                 // execute task … then report back
///                 registry.complete_run(&run_id, Ok("LGTM".to_string())).await;
///             })
///         }
///     })
///     .await?;
///
/// let result = registry
///     .wait_for_completion(&run_id, Duration::from_secs(30))
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct SubagentRegistry {
    runs: RwLock<HashMap<String, SubagentRun>>,
    metrics: RwLock<SubagentMetrics>,
    max_depth: u32,
    max_concurrent: usize,
}

impl SubagentRegistry {
    /// Create a new registry.
    ///
    /// * `max_depth` — maximum nesting depth (e.g. 3 means root, child, grandchild).
    /// * `max_concurrent` — maximum number of simultaneously *running* subagents.
    pub fn new(max_depth: u32, max_concurrent: usize) -> Self {
        Self {
            runs: RwLock::new(HashMap::new()),
            metrics: RwLock::new(SubagentMetrics::default()),
            max_depth,
            max_concurrent,
        }
    }

    // ------------------------------------------------------------------ //
    // Spawn                                                                //
    // ------------------------------------------------------------------ //

    /// Spawn a subagent to handle `task` on behalf of `parent_session`.
    ///
    /// The caller supplies a `task_fn` closure that receives the `run_id` and
    /// task string, and is responsible for executing the work and calling
    /// [`complete_run`](Self::complete_run) when done.
    ///
    /// Returns the `run_id` immediately; the task runs in the background.
    pub async fn spawn<F, Fut>(
        &self,
        parent_session: &str,
        target_agent: &str,
        task: &str,
        task_fn: F,
    ) -> crate::Result<String>
    where
        F: FnOnce(String, String) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        // ── depth check ──────────────────────────────────────────────────
        let current_depth = self.get_depth(parent_session).await;
        if current_depth >= self.max_depth {
            return Err(MantaError::MaxSpawnDepth(self.max_depth));
        }

        // ── concurrency check ────────────────────────────────────────────
        let active = self.active_count().await;
        if active >= self.max_concurrent {
            return Err(MantaError::MaxConcurrentSubagents(self.max_concurrent));
        }

        // ── register run ─────────────────────────────────────────────────
        let run_id = Uuid::new_v4().to_string();
        let child_session = format!("{}:subagent:{}", parent_session, &run_id[..8]);

        let run = SubagentRun {
            run_id: run_id.clone(),
            parent_session: parent_session.to_string(),
            child_session: child_session.clone(),
            target_agent: target_agent.to_string(),
            status: SubagentStatus::Running,
            spawn_depth: current_depth + 1,
            started_at: Instant::now(),
            completed_at: None,
        };

        {
            let mut runs = self.runs.write().await;
            runs.insert(run_id.clone(), run);
        }
        {
            let mut m = self.metrics.write().await;
            m.total_spawned += 1;
        }

        info!(
            run_id = %run_id,
            parent_session = %parent_session,
            target_agent = %target_agent,
            depth = current_depth + 1,
            "Subagent spawned"
        );

        // ── launch background task ───────────────────────────────────────
        let task_owned = task.to_string();
        tokio::spawn(task_fn(run_id.clone(), task_owned));

        Ok(run_id)
    }

    // ------------------------------------------------------------------ //
    // Completion                                                           //
    // ------------------------------------------------------------------ //

    /// Report the outcome of a run.  Call this from inside the `task_fn` you
    /// passed to [`spawn`](Self::spawn).
    pub async fn complete_run(&self, run_id: &str, result: Result<String, String>) {
        let mut runs = self.runs.write().await;
        let mut metrics = self.metrics.write().await;

        if let Some(run) = runs.get_mut(run_id) {
            run.completed_at = Some(Instant::now());
            match result {
                Ok(output) => {
                    run.status = SubagentStatus::Completed(output);
                    metrics.total_completed += 1;
                    debug!(run_id = %run_id, "Subagent completed successfully");
                }
                Err(err) => {
                    run.status = SubagentStatus::Failed(err.clone());
                    metrics.total_failed += 1;
                    error!(run_id = %run_id, error = %err, "Subagent failed");
                }
            }
        }
    }

    // ------------------------------------------------------------------ //
    // Wait                                                                 //
    // ------------------------------------------------------------------ //

    /// Block until the run finishes or `timeout` elapses.
    pub async fn wait_for_completion(
        &self,
        run_id: &str,
        timeout: Duration,
    ) -> crate::Result<String> {
        let deadline = Instant::now() + timeout;

        loop {
            if Instant::now() >= deadline {
                return Err(MantaError::SubagentTimeout);
            }

            {
                let runs = self.runs.read().await;
                match runs.get(run_id) {
                    None => return Err(MantaError::SubagentNotFound),
                    Some(run) => match &run.status {
                        SubagentStatus::Completed(out) => return Ok(out.clone()),
                        SubagentStatus::Failed(err) => {
                            return Err(MantaError::SubagentFailed(err.clone()))
                        }
                        SubagentStatus::Killed => return Err(MantaError::SubagentKilled),
                        SubagentStatus::Running => {}
                    },
                }
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    // ------------------------------------------------------------------ //
    // Kill                                                                 //
    // ------------------------------------------------------------------ //

    /// Mark a running subagent as killed.
    ///
    /// This sets the status flag; it does **not** abort the underlying Tokio
    /// task — the task_fn should poll the registry status and exit early if
    /// it observes `Killed`.
    pub async fn kill(&self, run_id: &str) -> crate::Result<()> {
        let mut runs = self.runs.write().await;
        let mut metrics = self.metrics.write().await;

        match runs.get_mut(run_id) {
            None => Err(MantaError::SubagentNotFound),
            Some(run) => {
                run.status = SubagentStatus::Killed;
                run.completed_at = Some(Instant::now());
                metrics.total_killed += 1;
                info!(run_id = %run_id, "Subagent killed");
                Ok(())
            }
        }
    }

    // ------------------------------------------------------------------ //
    // Queries                                                              //
    // ------------------------------------------------------------------ //

    /// Return a snapshot of a specific run, if it exists.
    pub async fn get_run(&self, run_id: &str) -> Option<SubagentRun> {
        self.runs.read().await.get(run_id).cloned()
    }

    /// Return all runs (running and finished) for a given parent session.
    pub async fn runs_for_session(&self, parent_session: &str) -> Vec<SubagentRun> {
        self.runs
            .read()
            .await
            .values()
            .filter(|r| r.parent_session == parent_session)
            .cloned()
            .collect()
    }

    /// Current snapshot of aggregate metrics.
    pub async fn metrics(&self) -> SubagentMetrics {
        self.metrics.read().await.clone()
    }

    /// Number of currently running subagents.
    pub async fn active_count(&self) -> usize {
        self.runs
            .read()
            .await
            .values()
            .filter(|r| matches!(r.status, SubagentStatus::Running))
            .count()
    }

    /// Determine the nesting depth for a (child) session by locating the run
    /// whose `child_session` matches it.
    async fn get_depth(&self, session: &str) -> u32 {
        self.runs
            .read()
            .await
            .values()
            .find(|r| r.child_session == session)
            .map(|r| r.spawn_depth)
            .unwrap_or(0)
    }

    /// Remove finished/killed runs older than `max_age`.  Call periodically to
    /// prevent unbounded memory growth.
    pub async fn cleanup(&self, max_age: Duration) {
        let cutoff = Instant::now()
            .checked_sub(max_age)
            .unwrap_or_else(Instant::now);

        let mut runs = self.runs.write().await;
        runs.retain(|_, run| {
            matches!(run.status, SubagentStatus::Running)
                || run.completed_at.map(|t| t > cutoff).unwrap_or(true)
        });
    }

    // ── Persistence ──────────────────────────────────────────────────────────

    /// Persist completed/failed runs to `path` as newline-delimited JSON.
    ///
    /// Running and killed runs are omitted — only terminal states are written
    /// so the file can be used to reconstruct crash history on restart.
    pub async fn persist_to(&self, path: &Path) -> std::io::Result<()> {
        let runs = self.runs.read().await;
        let now_instant = Instant::now();
        let now_system = SystemTime::now();

        let records: Vec<RunRecord> = runs
            .values()
            .filter_map(|r| {
                // Only persist terminal states.
                let outcome = match &r.status {
                    SubagentStatus::Completed(out) => RunOutcome::Completed(out.clone()),
                    SubagentStatus::Failed(err) => RunOutcome::Failed(err.clone()),
                    _ => return None,
                };

                // Convert Instant durations to SystemTime for serialization.
                let elapsed_started = now_instant
                    .checked_duration_since(r.started_at)
                    .unwrap_or_default();
                let started_at = now_system
                    .checked_sub(elapsed_started)
                    .unwrap_or(now_system);

                let elapsed_completed = r
                    .completed_at
                    .and_then(|t| now_instant.checked_duration_since(t))
                    .unwrap_or_default();
                let completed_at = now_system.checked_sub(elapsed_completed).unwrap_or(now_system);

                Some(RunRecord {
                    run_id: r.run_id.clone(),
                    parent_session: r.parent_session.clone(),
                    child_session: r.child_session.clone(),
                    target_agent: r.target_agent.clone(),
                    spawn_depth: r.spawn_depth,
                    started_at,
                    completed_at,
                    outcome,
                })
            })
            .collect();

        let json = serde_json::to_string(&records)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        tokio::fs::write(path, json).await?;
        debug!(path = %path.display(), count = records.len(), "Persisted subagent run records");
        Ok(())
    }

    /// Load crash-recovery records from `path`.
    ///
    /// Returns the deserialized records without inserting them into the
    /// live registry (they're historical, not active runs).
    pub async fn load_from(path: &Path) -> std::io::Result<Vec<RunRecord>> {
        let bytes = tokio::fs::read(path).await?;
        let records: Vec<RunRecord> = serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        info!(path = %path.display(), count = records.len(), "Loaded subagent run records");
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_spawn_and_complete() {
        let registry = Arc::new(SubagentRegistry::new(3, 10));
        let reg = Arc::clone(&registry);

        let run_id = registry
            .spawn("session-1", "test-agent", "do something", {
                let reg = Arc::clone(&reg);
                move |run_id, _task| async move {
                    reg.complete_run(&run_id, Ok("done".to_string())).await;
                }
            })
            .await
            .unwrap();

        let result = registry
            .wait_for_completion(&run_id, Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(result, "done");
    }

    #[tokio::test]
    async fn test_max_depth_enforced() {
        let registry = Arc::new(SubagentRegistry::new(1, 10));

        // First spawn at depth 1 — OK
        let reg = Arc::clone(&registry);
        let run_id = registry
            .spawn("root", "agent", "task", {
                let reg = Arc::clone(&reg);
                move |run_id, _| async move {
                    reg.complete_run(&run_id, Ok("ok".to_string())).await;
                }
            })
            .await
            .unwrap();

        // Wait so child_session is registered
        let _ = registry
            .wait_for_completion(&run_id, Duration::from_secs(5))
            .await;

        // Try to spawn from child session — should fail (depth >= max_depth=1)
        // child_session is "root:subagent:<short-run-id>"
        let child_session = {
            let runs = registry.runs.read().await;
            runs.get(&run_id).unwrap().child_session.clone()
        };

        let result = registry
            .spawn(&child_session, "agent", "nested", |_, _| async {})
            .await;

        assert!(matches!(result, Err(MantaError::MaxSpawnDepth(1))));
    }

    #[tokio::test]
    async fn test_max_concurrent_enforced() {
        let registry = Arc::new(SubagentRegistry::new(5, 2));

        // Fill up to limit — tasks won't complete so they stay Running
        for i in 0..2 {
            let reg = Arc::clone(&registry);
            registry
                .spawn(
                    &format!("session-{}", i),
                    "agent",
                    "blocking",
                    |_, _| async { /* never completes */ },
                )
                .await
                .unwrap();
        }

        let result = registry
            .spawn("session-overflow", "agent", "task", |_, _| async {})
            .await;

        assert!(matches!(result, Err(MantaError::MaxConcurrentSubagents(2))));
    }

    #[tokio::test]
    async fn test_kill() {
        let registry = Arc::new(SubagentRegistry::new(3, 10));
        let reg = Arc::clone(&registry);

        let run_id = registry
            .spawn("session-1", "agent", "task", |_, _| async { /* hangs */ })
            .await
            .unwrap();

        registry.kill(&run_id).await.unwrap();

        let result = registry
            .wait_for_completion(&run_id, Duration::from_millis(100))
            .await;

        assert!(matches!(result, Err(MantaError::SubagentKilled)));
    }

    #[tokio::test]
    async fn test_metrics() {
        let registry = Arc::new(SubagentRegistry::new(3, 10));
        let reg = Arc::clone(&registry);

        let run_id = registry
            .spawn("session-1", "agent", "task", {
                let reg = Arc::clone(&reg);
                move |run_id, _| async move {
                    reg.complete_run(&run_id, Ok("ok".to_string())).await;
                }
            })
            .await
            .unwrap();

        let _ = registry
            .wait_for_completion(&run_id, Duration::from_secs(5))
            .await;

        let m = registry.metrics().await;
        assert_eq!(m.total_spawned, 1);
        assert_eq!(m.total_completed, 1);
        assert_eq!(m.total_failed, 0);
    }

    #[tokio::test]
    async fn test_not_found() {
        let registry = SubagentRegistry::new(3, 10);
        let result = registry
            .wait_for_completion("no-such-id", Duration::from_millis(10))
            .await;
        assert!(matches!(result, Err(MantaError::SubagentNotFound)));
    }
}
