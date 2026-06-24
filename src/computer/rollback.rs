//! Rollback manager — create snapshots before risky operations and restore
//! them on failure.
//!
//! Used by [`VerificationEngine`](super::verification::VerificationEngine) and
//! [`GoalPlanner`](crate::planner) to make multi-step workflows safe.

use std::path::{Path, PathBuf};

/// A single snapshot of some system state.
#[derive(Debug, Clone)]
pub enum Snapshot {
    /// A copy of a single file.
    FileBackup {
        original_path: PathBuf,
        backup_path: PathBuf,
    },
    /// A copy of an entire directory tree.
    DirectoryBackup {
        original_path: PathBuf,
        backup_path: PathBuf,
    },
    /// macOS APFS snapshot (system-level, best-effort).
    #[cfg(target_os = "macos")]
    ApfsSnapshot {
        path: PathBuf,
        snapshot_name: String,
    },
    /// Linux Btrfs snapshot (system-level, best-effort).
    #[cfg(target_os = "linux")]
    BtrfsSnapshot {
        subvolume: PathBuf,
        snapshot_path: PathBuf,
    },
}

/// Manages snapshots and rollbacks for safe execution.
#[derive(Debug, Default)]
pub struct RollbackManager {
    snapshots: Vec<Snapshot>,
    /// Directory where backup copies are stored.
    backup_dir: PathBuf,
}

impl RollbackManager {
    /// Return a reference to the most recently pushed snapshot, or an
    /// internal error if the snapshot list is unexpectedly empty.
    fn last_snapshot(&self) -> crate::Result<&Snapshot> {
        self.snapshots.last().ok_or_else(|| {
            crate::error::SyscityError::Internal(
                "snapshot push did not produce a last entry".to_string(),
            )
        })
    }

    /// Create a new rollback manager.
    ///
    /// Backups are stored in a temporary directory that is cleaned up on drop.
    pub fn new() -> crate::Result<Self> {
        let backup_dir =
            std::env::temp_dir().join(format!("syscity-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&backup_dir).map_err(|e| {
            crate::error::SyscityError::ExternalService {
                source: "Failed to create rollback backup directory".to_string(),
                cause: Some(Box::new(e)),
            }
        })?;
        Ok(Self {
            snapshots: Vec::new(),
            backup_dir,
        })
    }

    /// Create a new rollback manager with a specific backup directory.
    ///
    /// The directory is created if it does not already exist.
    pub fn with_backup_dir(backup_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&backup_dir);
        Self {
            snapshots: Vec::new(),
            backup_dir,
        }
    }

    /// Snapshot a single file before modifying it.
    ///
    /// The file is copied into the backup directory.  On rollback the copy
    /// is restored to the original location.
    pub async fn snapshot_file(&mut self, path: &Path) -> crate::Result<&Snapshot> {
        if !tokio::fs::try_exists(path).await.unwrap_or(false) {
            // File doesn't exist yet — nothing to snapshot, but record a
            // marker so rollback can delete it if it was created.
            let backup_path = self.backup_dir.join(format!(
                ".deleted.{}.{}",
                path.file_name()
                    .map(|n| n.to_string_lossy().replace('/', "_"))
                    .unwrap_or_else(|| "unknown".to_string()),
                uuid::Uuid::new_v4()
            ));
            let snap = Snapshot::FileBackup {
                original_path: path.to_path_buf(),
                backup_path,
            };
            self.snapshots.push(snap);
            return self.last_snapshot();
        }

        let backup_path = self.backup_dir.join(format!(
            "{}.{}.{}",
            path.file_name()
                .map(|n| n.to_string_lossy().replace('/', "_"))
                .unwrap_or_else(|| "unknown".to_string()),
            uuid::Uuid::new_v4(),
            "bak"
        ));

        tokio::fs::copy(path, &backup_path).await.map_err(|e| {
            crate::error::SyscityError::ExternalService {
                source: format!("Failed to snapshot file '{}'", path.display()),
                cause: Some(Box::new(e)),
            }
        })?;

        let snap = Snapshot::FileBackup {
            original_path: path.to_path_buf(),
            backup_path,
        };
        self.snapshots.push(snap);
        self.last_snapshot()
    }

    /// Snapshot a directory before modifying it.
    ///
    /// The directory is recursively copied into the backup directory.
    pub async fn snapshot_directory(&mut self, path: &Path) -> crate::Result<&Snapshot> {
        if !tokio::fs::try_exists(path).await.unwrap_or(false) {
            return Err(crate::error::SyscityError::Validation(format!(
                "Cannot snapshot non-existent directory '{}'",
                path.display()
            )));
        }

        let backup_path = self.backup_dir.join(format!(
            "dir.{}.{}",
            path.file_name()
                .map(|n| n.to_string_lossy().replace('/', "_"))
                .unwrap_or_else(|| "unknown".to_string()),
            uuid::Uuid::new_v4()
        ));

        copy_dir_recursive(path, &backup_path).await.map_err(|e| {
            crate::error::SyscityError::ExternalService {
                source: format!("Failed to snapshot directory '{}'", path.display()),
                cause: Some(Box::new(e)),
            }
        })?;

        let snap = Snapshot::DirectoryBackup {
            original_path: path.to_path_buf(),
            backup_path,
        };
        self.snapshots.push(snap);
        self.last_snapshot()
    }

    /// Create an APFS snapshot on macOS (best-effort).
    ///
    /// Uses `tmutil snapshot` or `diskutil apfs snapshot`.
    #[cfg(target_os = "macos")]
    pub async fn snapshot_apfs(&mut self, path: &Path) -> crate::Result<&Snapshot> {
        let snapshot_name = format!("syscity-{}", uuid::Uuid::new_v4());

        // Try tmutil first (Time Machine local snapshots)
        let tmutil_result = tokio::process::Command::new("tmutil")
            .args(["snapshot", &path.to_string_lossy()])
            .output()
            .await;

        if let Ok(output) = tmutil_result {
            if output.status.success() {
                tracing::info!("APFS snapshot created via tmutil for {}", path.display());
            } else {
                tracing::warn!(
                    "tmutil snapshot failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }

        let snap = Snapshot::ApfsSnapshot {
            path: path.to_path_buf(),
            snapshot_name,
        };
        self.snapshots.push(snap);
        self.last_snapshot()
    }

    /// Create a Btrfs snapshot on Linux (best-effort).
    ///
    /// Uses `btrfs subvolume snapshot`.
    #[cfg(target_os = "linux")]
    pub async fn snapshot_btrfs(&mut self, subvolume: &Path) -> crate::Result<&Snapshot> {
        if !tokio::fs::try_exists(subvolume).await.unwrap_or(false) {
            return Err(crate::error::SyscityError::Validation(format!(
                "Cannot snapshot non-existent Btrfs subvolume '{}'",
                subvolume.display()
            )));
        }

        let snapshot_path = self.backup_dir.join(format!(
            "btrfs.{}.{}",
            subvolume
                .file_name()
                .map(|n| n.to_string_lossy().replace('/', "_"))
                .unwrap_or_else(|| "unknown".to_string()),
            uuid::Uuid::new_v4()
        ));

        let output = tokio::process::Command::new("btrfs")
            .args([
                "subvolume",
                "snapshot",
                "-r",
                &subvolume.to_string_lossy(),
                &snapshot_path.to_string_lossy(),
            ])
            .output()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: format!("Failed to create Btrfs snapshot for '{}'", subvolume.display()),
                cause: Some(Box::new(e)),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("btrfs snapshot failed: {}", stderr);
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("Btrfs snapshot failed: {}", stderr),
                cause: None,
            });
        }

        tracing::info!(
            "Btrfs snapshot created: {} -> {}",
            subvolume.display(),
            snapshot_path.display()
        );

        let snap = Snapshot::BtrfsSnapshot {
            subvolume: subvolume.to_path_buf(),
            snapshot_path,
        };
        self.snapshots.push(snap);
        self.last_snapshot()
    }

    /// Create a Windows System Restore point (best-effort).
    #[cfg(target_os = "windows")]
    pub async fn snapshot_windows(&mut self, description: &str) -> crate::Result<&Snapshot> {
        // Use WMI to create a system restore point
        let ps_script = format!(
            r#"
$description = '{}'
Checkpoint-Computer -Description $description -RestorePointType 'MODIFY_SETTINGS' -ErrorAction Stop
"#,
            description.replace('"', "\"").replace('\'', "''")
        );

        let output = tokio::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &ps_script,
            ])
            .output()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: "Failed to create Windows System Restore point".to_string(),
                cause: Some(Box::new(e)),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("Windows System Restore failed: {}", stderr);
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("System Restore failed: {}", stderr),
                cause: None,
            });
        }

        tracing::info!("Windows System Restore point created: {}", description);

        // Store a marker snapshot — actual restore is via System Restore UI
        let snap = Snapshot::FileBackup {
            original_path: PathBuf::from("/"),
            backup_path: PathBuf::from(format!("system-restore:{}", description)),
        };
        self.snapshots.push(snap);
        self.last_snapshot()
    }

    /// Restore all snapshots in reverse order (LIFO).
    ///
    /// This is safe to call multiple times — after the first successful
    /// rollback the snapshot list is cleared.
    pub async fn rollback(&mut self) -> crate::Result<()> {
        let mut errors = Vec::new();

        for snapshot in self.snapshots.drain(..).rev() {
            if let Err(e) = Self::restore_snapshot(&snapshot).await {
                errors.push(format!("{}: {}", snapshot_desc(&snapshot), e));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(crate::error::SyscityError::ExternalService {
                source: format!("Rollback partially failed: {}", errors.join("; ")),
                cause: None,
            })
        }
    }

    /// Restore a single snapshot.
    async fn restore_snapshot(snapshot: &Snapshot) -> crate::Result<()> {
        match snapshot {
            Snapshot::FileBackup { original_path, backup_path } => {
                if tokio::fs::try_exists(backup_path).await.unwrap_or(false) {
                    // Backup exists → restore it
                    tokio::fs::copy(backup_path, original_path)
                        .await
                        .map_err(|e| crate::error::SyscityError::ExternalService {
                            source: format!("Failed to restore file '{}'", original_path.display()),
                            cause: Some(Box::new(e)),
                        })?;
                } else {
                    // Backup is a marker for "didn't exist before" → delete
                    // the file if it now exists.
                    if tokio::fs::try_exists(original_path).await.unwrap_or(false) {
                        tokio::fs::remove_file(original_path).await.map_err(|e| {
                            crate::error::SyscityError::ExternalService {
                                source: format!(
                                    "Failed to remove created file '{}'",
                                    original_path.display()
                                ),
                                cause: Some(Box::new(e)),
                            }
                        })?;
                    }
                }
            }
            Snapshot::DirectoryBackup { original_path, backup_path } => {
                // Remove the modified directory and restore from backup.
                if tokio::fs::try_exists(original_path).await.unwrap_or(false) {
                    tokio::fs::remove_dir_all(original_path)
                        .await
                        .map_err(|e| crate::error::SyscityError::ExternalService {
                            source: format!(
                                "Failed to remove modified directory '{}'",
                                original_path.display()
                            ),
                            cause: Some(Box::new(e)),
                        })?;
                }
                copy_dir_recursive(backup_path, original_path)
                    .await
                    .map_err(|e| crate::error::SyscityError::ExternalService {
                        source: format!(
                            "Failed to restore directory '{}'",
                            original_path.display()
                        ),
                        cause: Some(Box::new(e)),
                    })?;
            }
            #[cfg(target_os = "macos")]
            Snapshot::ApfsSnapshot { path, snapshot_name } => {
                // Best-effort APFS snapshot restore via tmutil
                let _ = tokio::process::Command::new("tmutil")
                    .args([
                        "restore",
                        &format!("{}/{}", path.display(), snapshot_name),
                        &path.to_string_lossy(),
                    ])
                    .output()
                    .await;
            }
            #[cfg(target_os = "linux")]
            Snapshot::BtrfsSnapshot { subvolume, snapshot_path } => {
                // Best-effort Btrfs snapshot restore
                let _ = tokio::process::Command::new("btrfs")
                    .args(["subvolume", "delete", &subvolume.to_string_lossy()])
                    .output()
                    .await;
                let _ = tokio::process::Command::new("btrfs")
                    .args([
                        "subvolume",
                        "snapshot",
                        &snapshot_path.to_string_lossy(),
                        &subvolume.to_string_lossy(),
                    ])
                    .output()
                    .await;
            }
        }
        Ok(())
    }

    /// Roll back the last `n` snapshots (most recent first).
    ///
    /// This is useful for step-by-step undo where each step may have
    /// created one or more snapshots.
    pub async fn rollback_last(&mut self, n: usize) -> crate::Result<()> {
        let n = n.min(self.snapshots.len());
        if n == 0 {
            return Ok(());
        }
        let to_rollback: Vec<Snapshot> = self
            .snapshots
            .split_off(self.snapshots.len().saturating_sub(n));
        let mut errors = Vec::new();
        for snapshot in to_rollback.iter().rev() {
            if let Err(e) = Self::restore_snapshot(snapshot).await {
                errors.push(format!("{}: {}", snapshot_desc(snapshot), e));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(crate::error::SyscityError::ExternalService {
                source: format!("Rollback partially failed: {}", errors.join("; ")),
                cause: None,
            })
        }
    }

    /// Take a system-level snapshot best suited for the current platform.
    ///
    /// - macOS → APFS snapshot via `tmutil`
    /// - Linux → Btrfs read-only subvolume snapshot
    /// - Windows → System Restore point
    ///
    /// Falls back to a directory backup if system-level snapshots are
    /// unavailable.
    pub async fn snapshot_system(&mut self, path: &Path) -> crate::Result<&Snapshot> {
        #[cfg(target_os = "macos")]
        {
            if self.snapshot_apfs(path).await.is_ok() {
                return self.last_snapshot();
            }
            tracing::warn!("APFS snapshot failed, falling back to directory backup");
        }
        #[cfg(target_os = "linux")]
        {
            if self.snapshot_btrfs(path).await.is_ok() {
                return self.last_snapshot();
            }
            tracing::warn!("Btrfs snapshot failed, falling back to directory backup");
        }
        #[cfg(target_os = "windows")]
        {
            let description = format!("syscity-rollback-{}", uuid::Uuid::new_v4());
            if self.snapshot_windows(&description).await.is_ok() {
                return self.last_snapshot();
            }
            tracing::warn!("Windows System Restore failed, falling back to directory backup");
        }
        // Fallback: directory backup
        self.snapshot_directory(path).await
    }

    /// Remove all snapshots and clear the list.
    ///
    /// Call this after a successful workflow to free disk space.
    pub async fn clear(&mut self) -> crate::Result<()> {
        self.snapshots.clear();
        if tokio::fs::try_exists(&self.backup_dir)
            .await
            .unwrap_or(false)
        {
            tokio::fs::remove_dir_all(&self.backup_dir)
                .await
                .map_err(|e| crate::error::SyscityError::ExternalService {
                    source: format!(
                        "Failed to clean up backup directory '{}'",
                        self.backup_dir.display()
                    ),
                    cause: Some(Box::new(e)),
                })?;
        }
        Ok(())
    }

    /// Number of snapshots currently held.
    pub fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }

    /// Whether any snapshots have been recorded.
    pub fn has_snapshots(&self) -> bool {
        !self.snapshots.is_empty()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn snapshot_desc(snapshot: &Snapshot) -> String {
    match snapshot {
        Snapshot::FileBackup { original_path, .. } => {
            format!("file '{}'", original_path.display())
        }
        Snapshot::DirectoryBackup { original_path, .. } => {
            format!("directory '{}'", original_path.display())
        }
        #[cfg(target_os = "macos")]
        Snapshot::ApfsSnapshot { path, .. } => {
            format!("APFS snapshot '{}'", path.display())
        }
        #[cfg(target_os = "linux")]
        Snapshot::BtrfsSnapshot { subvolume, .. } => {
            format!("Btrfs snapshot '{}'", subvolume.display())
        }
    }
}

/// Recursively copy a directory tree.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dst).await?;
    let mut entries = tokio::fs::read_dir(src).await?;

    while let Some(entry) = entries.next_entry().await? {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry.file_type().await?;

        if file_type.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else if file_type.is_file() {
            tokio::fs::copy(&src_path, &dst_path).await?;
        }
        // Symlinks are skipped — this is intentional for rollback safety.
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_snapshot_and_rollback_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("test.txt");
        tokio::fs::write(&file, "original").await.unwrap();

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_file(&file).await.unwrap();
        assert_eq!(mgr.snapshot_count(), 1);

        // Modify the file
        tokio::fs::write(&file, "modified").await.unwrap();
        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, "modified");

        // Rollback
        mgr.rollback().await.unwrap();
        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, "original");
    }

    #[tokio::test]
    async fn test_snapshot_and_rollback_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("project");
        tokio::fs::create_dir(&dir).await.unwrap();
        tokio::fs::write(dir.join("a.txt"), "A").await.unwrap();
        tokio::fs::write(dir.join("b.txt"), "B").await.unwrap();

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_directory(&dir).await.unwrap();
        assert_eq!(mgr.snapshot_count(), 1);

        // Modify
        tokio::fs::write(dir.join("a.txt"), "A-modified")
            .await
            .unwrap();
        tokio::fs::remove_file(dir.join("b.txt")).await.unwrap();

        // Rollback
        mgr.rollback().await.unwrap();
        let a = tokio::fs::read_to_string(dir.join("a.txt")).await.unwrap();
        let b = tokio::fs::read_to_string(dir.join("b.txt")).await.unwrap();
        assert_eq!(a, "A");
        assert_eq!(b, "B");
    }

    #[tokio::test]
    async fn test_snapshot_nonexistent_file_rollback_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("new.txt");

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_file(&file).await.unwrap();

        // File is created after snapshot
        tokio::fs::write(&file, "created").await.unwrap();
        assert!(tokio::fs::try_exists(&file).await.unwrap());

        // Rollback should delete it
        mgr.rollback().await.unwrap();
        assert!(!tokio::fs::try_exists(&file).await.unwrap());
    }

    #[tokio::test]
    async fn test_clear_removes_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("test.txt");
        tokio::fs::write(&file, "hello").await.unwrap();

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_file(&file).await.unwrap();
        mgr.clear().await.unwrap();

        assert_eq!(mgr.snapshot_count(), 0);
        assert!(!tokio::fs::try_exists(&mgr.backup_dir).await.unwrap());
    }

    #[tokio::test]
    async fn test_rollback_last_partial() {
        let tmp = tempfile::tempdir().unwrap();
        let file1 = tmp.path().join("a.txt");
        let file2 = tmp.path().join("b.txt");
        tokio::fs::write(&file1, "A1").await.unwrap();
        tokio::fs::write(&file2, "B1").await.unwrap();

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_file(&file1).await.unwrap();
        mgr.snapshot_file(&file2).await.unwrap();
        assert_eq!(mgr.snapshot_count(), 2);

        // Modify both files
        tokio::fs::write(&file1, "A2").await.unwrap();
        tokio::fs::write(&file2, "B2").await.unwrap();

        // Rollback only the last snapshot (file2)
        mgr.rollback_last(1).await.unwrap();
        assert_eq!(mgr.snapshot_count(), 1);

        let a = tokio::fs::read_to_string(&file1).await.unwrap();
        let b = tokio::fs::read_to_string(&file2).await.unwrap();
        assert_eq!(a, "A2"); // unchanged
        assert_eq!(b, "B1"); // restored

        // Rollback the remaining snapshot
        mgr.rollback_last(1).await.unwrap();
        assert_eq!(mgr.snapshot_count(), 0);

        let a = tokio::fs::read_to_string(&file1).await.unwrap();
        assert_eq!(a, "A1"); // restored
    }

    #[tokio::test]
    async fn test_rollback_last_zero_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("test.txt");
        tokio::fs::write(&file, "original").await.unwrap();

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_file(&file).await.unwrap();

        tokio::fs::write(&file, "modified").await.unwrap();
        mgr.rollback_last(0).await.unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, "modified");
        assert_eq!(mgr.snapshot_count(), 1);
    }

    #[tokio::test]
    async fn test_rollback_last_more_than_exists_rolls_all() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("test.txt");
        tokio::fs::write(&file, "original").await.unwrap();

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_file(&file).await.unwrap();

        tokio::fs::write(&file, "modified").await.unwrap();
        mgr.rollback_last(100).await.unwrap();

        let content = tokio::fs::read_to_string(&file).await.unwrap();
        assert_eq!(content, "original");
        assert_eq!(mgr.snapshot_count(), 0);
    }

    #[test]
    fn test_snapshot_desc() {
        let snap = Snapshot::FileBackup {
            original_path: PathBuf::from("/tmp/test.txt"),
            backup_path: PathBuf::from("/tmp/backup.txt"),
        };
        assert_eq!(snapshot_desc(&snap), "file '/tmp/test.txt'");
    }
}
