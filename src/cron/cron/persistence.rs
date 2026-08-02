//! `CronScheduler` job-store persistence: loading jobs on startup and atomic
//! save-on-change.

use super::*;

impl CronScheduler {
    /// Load jobs from store
    pub(super) async fn load_jobs(&mut self, path: &PathBuf) -> Result<()> {
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
    pub(super) async fn save_jobs(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        path: &PathBuf,
    ) -> Result<()> {
        // Scope the read lock to the data-collection phase only. Serialization
        // is fast, but filesystem I/O may block for milliseconds; dropping the
        // lock before I/O means Add/Remove/SetEnabled are never blocked behind
        // a write-to-disk.
        let json = {
            let jobs_lock = jobs.read().await;
            let jobs_vec: Vec<&CronJob> = jobs_lock.values().collect();
            serde_json::to_string_pretty(&jobs_vec)
                .map_err(|e| SyscityError::Internal(format!("Failed to serialize jobs: {}", e)))?
        };

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
}
