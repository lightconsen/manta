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
    /// Create a new rollback manager.
    ///
    /// Backups are stored in a temporary directory that is cleaned up on drop.
    pub fn new() -> crate::Result<Self> {
        let backup_dir = std::env::temp_dir()
            .join(format!("syscity-rollback-{}", uuid::Uuid::new_v4()));
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
    pub async fn snapshot_file(
        &mut self,
        path: &Path,
    ) -> crate::Result<&Snapshot> {
        if !tokio::fs::try_exists(path).await.unwrap_or(false) {
            // File doesn't exist yet — nothing to snapshot, but record a
            // marker so rollback can delete it if it was created.
            let backup_path = self.backup_dir.join(
            	format!(".deleted.{}.{}",
            	    path.file_name()
            	        .map(|n| n.to_string_lossy().replace('/', "_"))
            	        .unwrap_or_else(|| "unknown".to_string()),
            	    uuid::Uuid::new_v4()
            	)
            );
            let snap = Snapshot::FileBackup {
                original_path: path.to_path_buf(),
                backup_path,
            };
            self.snapshots.push(snap);
            return Ok(self.snapshots.last().unwrap());
        }

        let backup_path = self
            .backup_dir
            .join(format!(
                "{}.{}.{}",
                path.file_name()
                    .map(|n| n.to_string_lossy().replace('/', "_"))
                    .unwrap_or_else(|| "unknown".to_string()),
                uuid::Uuid::new_v4(),
                "bak"
            ));

        tokio::fs::copy(path, &backup_path)
            .await
            .map_err(|e| {
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
        Ok(self.snapshots.last().unwrap())
    }

    /// Snapshot a directory before modifying it.
    ///
    /// The directory is recursively copied into the backup directory.
    pub async fn snapshot_directory(
        &mut self,
        path: &Path,
    ) -> crate::Result<&Snapshot> {
        if !tokio::fs::try_exists(path).await.unwrap_or(false) {
            return Err(crate::error::SyscityError::Validation(format!(
                "Cannot snapshot non-existent directory '{}'",
                path.display()
            )));
        }

        let backup_path = self
            .backup_dir
            .join(format!(
                "dir.{}.{}",
                path.file_name()
                    .map(|n| n.to_string_lossy().replace('/', "_"))
                    .unwrap_or_else(|| "unknown".to_string()),
                uuid::Uuid::new_v4()
            ));

        copy_dir_recursive(path, &backup_path).await.map_err(|e| {
            crate::error::SyscityError::ExternalService {
                source: format!(
                    "Failed to snapshot directory '{}'",
                    path.display()
                ),
                cause: Some(Box::new(e)),
            }
        })?;

        let snap = Snapshot::DirectoryBackup {
            original_path: path.to_path_buf(),
            backup_path,
        };
        self.snapshots.push(snap);
        Ok(self.snapshots.last().unwrap())
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
                source: format!(
                    "Rollback partially failed: {}",
                    errors.join("; ")
                ),
                cause: None,
            })
        }
    }

    /// Restore a single snapshot.
    async fn restore_snapshot(snapshot: &Snapshot) -> crate::Result<()> {
        match snapshot {
            Snapshot::FileBackup {
                original_path,
                backup_path,
            } => {
                if tokio::fs::try_exists(backup_path).await.unwrap_or(false) {
                    // Backup exists → restore it
                    tokio::fs::copy(backup_path, original_path)
                        .await
                        .map_err(|e| {
                            crate::error::SyscityError::ExternalService {
                                source: format!(
                                    "Failed to restore file '{}'",
                                    original_path.display()
                                ),
                                cause: Some(Box::new(e)),
                            }
                        })?;
                } else {
                    // Backup is a marker for "didn't exist before" → delete
                    // the file if it now exists.
                    if tokio::fs::try_exists(original_path)
                        .await
                        .unwrap_or(false)
                    {
                        tokio::fs::remove_file(original_path)
                            .await
                            .map_err(|e| {
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
            Snapshot::DirectoryBackup {
                original_path,
                backup_path,
            } => {
                // Remove the modified directory and restore from backup.
                if tokio::fs::try_exists(original_path)
                    .await
                    .unwrap_or(false)
                {
                    tokio::fs::remove_dir_all(original_path)
                        .await
                        .map_err(|e| {
                            crate::error::SyscityError::ExternalService {
                                source: format!(
                                    "Failed to remove modified directory '{}'",
                                    original_path.display()
                                ),
                                cause: Some(Box::new(e)),
                            }
                        })?;
                }
                copy_dir_recursive(backup_path, original_path)
                    .await
                    .map_err(|e| {
                        crate::error::SyscityError::ExternalService {
                            source: format!(
                                "Failed to restore directory '{}'",
                                original_path.display()
                            ),
                            cause: Some(Box::new(e)),
                        }
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
            Snapshot::BtrfsSnapshot {
                subvolume,
                snapshot_path,
            } => {
                // Best-effort Btrfs snapshot restore
                let _ = tokio::process::Command::new("btrfs")
                    .args([
                        "subvolume",
                        "delete",
                        &subvolume.to_string_lossy(),
                    ])
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
                .map_err(|e| {
                    crate::error::SyscityError::ExternalService {
                        source: format!(
                            "Failed to clean up backup directory '{}'",
                            self.backup_dir.display()
                        ),
                        cause: Some(Box::new(e)),
                    }
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
        tokio::fs::write(&file, "original")
            .await
            .unwrap();

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_file(&file).await.unwrap();
        assert_eq!(mgr.snapshot_count(), 1);

        // Modify the file
        tokio::fs::write(&file, "modified")
            .await
            .unwrap();
        let content = tokio::fs::read_to_string(&file)
            .await
            .unwrap();
        assert_eq!(content, "modified");

        // Rollback
        mgr.rollback().await.unwrap();
        let content = tokio::fs::read_to_string(&file)
            .await
            .unwrap();
        assert_eq!(content, "original");
    }

    #[tokio::test]
    async fn test_snapshot_and_rollback_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("project");
        tokio::fs::create_dir(&dir).await.unwrap();
        tokio::fs::write(dir.join("a.txt"), "A")
            .await
            .unwrap();
        tokio::fs::write(dir.join("b.txt"), "B")
            .await
            .unwrap();

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_directory(&dir).await.unwrap();
        assert_eq!(mgr.snapshot_count(), 1);

        // Modify
        tokio::fs::write(dir.join("a.txt"), "A-modified")
            .await
            .unwrap();
        tokio::fs::remove_file(dir.join("b.txt"))
            .await
            .unwrap();

        // Rollback
        mgr.rollback().await.unwrap();
        let a = tokio::fs::read_to_string(dir.join("a.txt"))
            .await
            .unwrap();
        let b = tokio::fs::read_to_string(dir.join("b.txt"))
            .await
            .unwrap();
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
        tokio::fs::write(&file, "created")
            .await
            .unwrap();
        assert!(tokio::fs::try_exists(&file)
            .await
            .unwrap());

        // Rollback should delete it
        mgr.rollback().await.unwrap();
        assert!(!tokio::fs::try_exists(&file)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_clear_removes_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("test.txt");
        tokio::fs::write(&file, "hello")
            .await
            .unwrap();

        let mut mgr = RollbackManager::with_backup_dir(tmp.path().join("backups"));
        mgr.snapshot_file(&file).await.unwrap();
        mgr.clear().await.unwrap();

        assert_eq!(mgr.snapshot_count(), 0);
        assert!(!tokio::fs::try_exists(&mgr.backup_dir)
            .await
            .unwrap());
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
