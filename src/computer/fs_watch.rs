//! Generic file system watcher for desktop agents.
//!
//! Unlike [`crate::skills::watcher::SkillWatcher`] (skill-specific hot-reload),
//! `FileWatcher` is a general-purpose wrapper around the `notify` crate that
//! can be used by any [`ComputerAdapter`] to watch arbitrary files and
//! directories.
//!
//! # Usage
//!
//! ```rust,no_run
//! use syscity::computer::fs_watch::FileWatcher;
//!
//! let mut watcher = FileWatcher::new().unwrap();
//! watcher.watch_directory("/tmp").unwrap();
//!
//! if let Some(change) = watcher.try_recv() {
//!     println!("{:?} changed: {:?}", change.path, change.kind);
//! }
//! ```

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// A single file-system change event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChangeEvent {
    /// Absolute path that changed.
    pub path: PathBuf,
    /// Kind of change.
    pub kind: FileChangeKind,
}

/// Type of file-system change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeKind {
    /// File or directory was created.
    Created,
    /// File or directory was modified.
    Modified,
    /// File or directory was removed.
    Removed,
    /// Multiple or unknown changes (rename, etc.).
    Mixed,
}

impl From<&notify::EventKind> for FileChangeKind {
    fn from(kind: &notify::EventKind) -> Self {
        use notify::EventKind::*;
        match kind {
            Create(_) => FileChangeKind::Created,
            Modify(_) => FileChangeKind::Modified,
            Remove(_) => FileChangeKind::Removed,
            _ => FileChangeKind::Mixed,
        }
    }
}

/// Generic file-system watcher backed by `notify`.
pub struct FileWatcher {
    /// Underlying OS watcher.
    watcher: RecommendedWatcher,
    /// Channel receiver for change events.
    rx: mpsc::UnboundedReceiver<FileChangeEvent>,
    /// Set of currently watched paths (tracked for rebuilds).
    watched_paths: HashSet<PathBuf>,
    /// Sender clone kept for rebuilds.
    _tx: mpsc::UnboundedSender<FileChangeEvent>,
}

impl FileWatcher {
    /// Create a new file watcher.
    pub fn new() -> crate::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let tx_clone = tx.clone();

        let watcher = notify::recommended_watcher(
            move |res: std::result::Result<Event, notify::Error>| match res {
                Ok(event) => {
                    let kind: FileChangeKind = (&event.kind).into();
                    for path in event.paths {
                        if let Err(e) = tx_clone.send(FileChangeEvent {
                            path: path.clone(),
                            kind,
                        }) {
                            warn!("Failed to send file change event: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("File watcher error: {}", e);
                }
            },
        )
        .map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "Failed to create file watcher: {}",
                e
            ))
        })?;

        Ok(Self {
            watcher,
            rx,
            watched_paths: HashSet::new(),
            _tx: tx,
        })
    }

    /// Watch a directory recursively.
    pub fn watch_directory(&mut self, path: impl AsRef<Path>) -> crate::Result<()> {
        let path = path.as_ref().to_path_buf();
        if self.watched_paths.contains(&path) {
            debug!("Already watching directory: {:?}", path);
            return Ok(());
        }

        self.watcher
            .watch(&path, RecursiveMode::Recursive)
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!(
                    "Failed to watch directory {:?}: {}",
                    path, e
                ))
            })?;

        self.watched_paths.insert(path.clone());
        info!("Watching directory: {:?}", path);
        Ok(())
    }

    /// Stop watching a directory.
    pub fn unwatch_directory(&mut self, path: impl AsRef<Path>) -> crate::Result<()> {
        let path = path.as_ref().to_path_buf();
        if !self.watched_paths.contains(&path) {
            return Ok(());
        }

        self.watcher.unwatch(&path).map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "Failed to unwatch directory {:?}: {}",
                path, e
            ))
        })?;

        self.watched_paths.remove(&path);
        info!("Stopped watching directory: {:?}", path);
        Ok(())
    }

    /// Watch a single file (non-recursive).
    pub fn watch_file(&mut self, path: impl AsRef<Path>) -> crate::Result<()> {
        let path = path.as_ref().to_path_buf();
        if self.watched_paths.contains(&path) {
            debug!("Already watching file: {:?}", path);
            return Ok(());
        }

        self.watcher.watch(&path, RecursiveMode::NonRecursive).map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "Failed to watch file {:?}: {}",
                path, e
            ))
        })?;

        self.watched_paths.insert(path.clone());
        info!("Watching file: {:?}", path);
        Ok(())
    }

    /// Stop watching a single file.
    pub fn unwatch_file(&mut self, path: impl AsRef<Path>) -> crate::Result<()> {
        let path = path.as_ref().to_path_buf();
        if !self.watched_paths.contains(&path) {
            return Ok(());
        }

        self.watcher.unwatch(&path).map_err(|e| {
            crate::error::SyscityError::Internal(format!(
                "Failed to unwatch file {:?}: {}",
                path, e
            ))
        })?;

        self.watched_paths.remove(&path);
        info!("Stopped watching file: {:?}", path);
        Ok(())
    }

    /// Get the next change event without blocking.
    pub fn try_recv(&mut self) -> Option<FileChangeEvent> {
        self.rx.try_recv().ok()
    }

    /// Get the next change event (async).
    pub async fn recv(&mut self) -> Option<FileChangeEvent> {
        self.rx.recv().await
    }

    /// Returns `true` if the given path is being watched.
    pub fn is_watching(&self, path: impl AsRef<Path>) -> bool {
        self.watched_paths.contains(path.as_ref())
    }

    /// Returns a snapshot of all watched paths.
    pub fn watched_paths(&self) -> Vec<PathBuf> {
        self.watched_paths.iter().cloned().collect()
    }

    /// Number of watched paths.
    pub fn len(&self) -> usize {
        self.watched_paths.len()
    }

    /// Returns `true` if no paths are being watched.
    pub fn is_empty(&self) -> bool {
        self.watched_paths.is_empty()
    }
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new().expect("Failed to create default FileWatcher")
    }
}

/// Result returned by watch / unwatch desktop actions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileWatchResult {
    pub path: String,
    pub success: bool,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_file_watcher_new() {
        let watcher = FileWatcher::new();
        assert!(watcher.is_ok());
    }

    #[test]
    fn test_file_change_kind_from_notify() {
        use notify::event::{CreateKind, ModifyKind, RemoveKind};
        use notify::EventKind;

        assert_eq!(
            FileChangeKind::from(&EventKind::Create(CreateKind::File)),
            FileChangeKind::Created
        );
        assert_eq!(
            FileChangeKind::from(&EventKind::Modify(ModifyKind::Any)),
            FileChangeKind::Modified
        );
        assert_eq!(
            FileChangeKind::from(&EventKind::Remove(RemoveKind::File)),
            FileChangeKind::Removed
        );
        assert_eq!(
            FileChangeKind::from(&EventKind::Other),
            FileChangeKind::Mixed
        );
    }

    #[tokio::test]
    async fn test_watch_and_detect_file_change() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");

        let mut watcher = FileWatcher::new().unwrap();
        watcher.watch_directory(temp_dir.path()).unwrap();

        // Create a file to trigger an event
        tokio::fs::write(&file_path, "hello").await.unwrap();

        // Wait for the event with timeout
        let event = tokio::time::timeout(Duration::from_secs(5), watcher.recv()).await;

        assert!(
            event.is_ok(),
            "Should receive a file change event within timeout"
        );
        let event = event.unwrap();
        assert!(event.is_some(), "Should have received an event");
        let event = event.unwrap();
        assert_eq!(event.path.file_name().unwrap(), "test.txt");
    }

    #[test]
    fn test_is_watching() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut watcher = FileWatcher::new().unwrap();

        assert!(!watcher.is_watching(temp_dir.path()));
        watcher.watch_directory(temp_dir.path()).unwrap();
        assert!(watcher.is_watching(temp_dir.path()));
    }

    #[test]
    fn test_unwatch_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut watcher = FileWatcher::new().unwrap();

        watcher.watch_directory(temp_dir.path()).unwrap();
        assert_eq!(watcher.len(), 1);

        watcher.unwatch_directory(temp_dir.path()).unwrap();
        assert_eq!(watcher.len(), 0);
        assert!(!watcher.is_watching(temp_dir.path()));
    }

    #[test]
    fn test_watch_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("watch_me.txt");
        std::fs::write(&file_path, "x").unwrap();

        let mut watcher = FileWatcher::new().unwrap();
        watcher.watch_file(&file_path).unwrap();

        assert!(watcher.is_watching(&file_path));
        assert_eq!(watcher.len(), 1);
    }

    #[test]
    fn test_file_watch_result_serde() {
        let result = FileWatchResult {
            path: "/tmp/test".to_string(),
            success: true,
            message: "ok".to_string(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("/tmp/test"));
    }
}
