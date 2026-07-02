//! File watcher for hot reloading skills
//!
//! Watches skill directories for changes and triggers reloads.
//! Uses the `notify` crate for cross-platform file system events.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

use super::storage::StorageLevel;

/// File change event
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path that changed
    pub path: PathBuf,
    /// Kind of change
    pub kind: ChangeKind,
}

/// Type of file change
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// File was created
    Created,
    /// File was modified
    Modified,
    /// File was removed
    Removed,
    /// Multiple changes (rename, etc.)
    Mixed,
}

impl From<&notify::EventKind> for ChangeKind {
    fn from(kind: &notify::EventKind) -> Self {
        use notify::EventKind::*;

        match kind {
            Create(_) => ChangeKind::Created,
            Modify(_) => ChangeKind::Modified,
            Remove(_) => ChangeKind::Removed,
            _ => ChangeKind::Mixed,
        }
    }
}

/// Callback type for file changes
pub type FileChangeCallback = Box<dyn Fn(String) + Send + Sync>;

/// Skill file watcher for hot reloading
pub struct SkillWatcher {
    /// Active watcher instance (replaced on rebuild)
    watcher: RecommendedWatcher,
    /// Sender end kept to create new watcher closures on rebuild
    tx: mpsc::UnboundedSender<FileChange>,
    /// Channel for change events
    rx: mpsc::UnboundedReceiver<FileChange>,
    /// Set of watched paths
    watched_paths: Arc<RwLock<HashSet<PathBuf>>>,
    /// Callback for file changes (used by rebuild_watcher)
    callback: Arc<RwLock<Option<FileChangeCallback>>>,
}

impl SkillWatcher {
    /// Create a new skill watcher with paths and callback
    pub fn new<F>(paths: Vec<(StorageLevel, PathBuf)>, callback: F) -> crate::Result<Self>
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::unbounded_channel();
        let watched_paths = Arc::new(RwLock::new(HashSet::new()));

        let callback_arc: Arc<RwLock<Option<FileChangeCallback>>> =
            Arc::new(RwLock::new(Some(Box::new(callback))));

        let tx_clone = tx.clone();
        let cb = Arc::clone(&callback_arc);
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    debug!("File system event: {:?}", event);

                    let kind: ChangeKind = (&event.kind).into();

                    for path in event.paths {
                        // Only report SKILL.md files
                        if path.to_string_lossy().ends_with("SKILL.md") {
                            if let Err(e) = tx_clone.send(FileChange { path: path.clone(), kind }) {
                                warn!("Failed to send file change event: {}", e);
                            }
                            // Also call the callback with the path
                            if let Ok(guard) = cb.try_read() {
                                if let Some(ref cb_fn) = *guard {
                                    cb_fn(path.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("File watcher error: {}", e);
                }
            }
        })
        .map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to create file watcher: {}", e))
        })?;

        // Watch all provided paths
        for (_level, path) in paths {
            if path.exists() {
                if let Err(e) = watcher.watch(&path, RecursiveMode::Recursive) {
                    warn!("Failed to watch path {:?}: {}", path, e);
                } else {
                    info!("Watching skill directory: {:?}", path);
                }
            }
        }

        Ok(Self {
            watcher,
            tx,
            rx,
            watched_paths,
            callback: callback_arc,
        })
    }

    /// Watch a directory recursively
    pub async fn watch_dir(&mut self, path: &Path) -> crate::Result<()> {
        let mut paths = self.watched_paths.write().await;

        if paths.contains(path) {
            debug!("Already watching: {:?}", path);
            return Ok(());
        }

        info!("Starting to watch skill directory: {:?}", path);

        // Note: We can't easily add paths to an existing notify watcher
        // In a real implementation, we'd need to handle this differently
        // For now, we track what we want to watch

        paths.insert(path.to_path_buf());

        // Create the watcher fresh with all paths
        drop(paths);
        self.rebuild_watcher().await?;

        Ok(())
    }

    /// Stop watching a directory
    pub async fn unwatch_dir(&mut self, path: &Path) -> crate::Result<()> {
        let mut paths = self.watched_paths.write().await;

        if paths.remove(path) {
            info!("Stopped watching: {:?}", path);
        }

        drop(paths);
        self.rebuild_watcher().await?;

        Ok(())
    }

    /// Rebuild the watcher with current paths, replacing the OS watcher.
    async fn rebuild_watcher(&mut self) -> crate::Result<()> {
        let paths = self.watched_paths.read().await;
        debug!("Rebuilding watcher with {} paths", paths.len());

        let tx_clone = self.tx.clone();
        let cb = Arc::clone(&self.callback);
        let mut new_watcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    let kind: ChangeKind = (&event.kind).into();
                    for path in event.paths {
                        if path.to_string_lossy().ends_with("SKILL.md") {
                            if let Err(e) = tx_clone.send(FileChange { path: path.clone(), kind }) {
                                warn!("Failed to send file change event: {}", e);
                            }
                            if let Ok(guard) = cb.try_read() {
                                if let Some(ref cb_fn) = *guard {
                                    cb_fn(path.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
                Err(e) => error!("File watcher error: {}", e),
            })
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!(
                    "Failed to recreate file watcher: {}",
                    e
                ))
            })?;

        for path in paths.iter() {
            if path.exists() {
                if let Err(e) = new_watcher.watch(path, RecursiveMode::Recursive) {
                    warn!("Failed to re-watch path {:?}: {}", path, e);
                } else {
                    info!("Re-watching skill directory: {:?}", path);
                }
            }
        }

        self.watcher = new_watcher;
        Ok(())
    }

    /// Get the next file change event (non-blocking)
    pub fn try_recv(&mut self) -> Option<FileChange> {
        self.rx.try_recv().ok()
    }

    /// Get the next file change event (blocking)
    pub async fn recv(&mut self) -> Option<FileChange> {
        self.rx.recv().await
    }

    /// Check if a path is being watched
    pub async fn is_watching(&self, path: &Path) -> bool {
        let paths = self.watched_paths.read().await;
        paths.contains(path)
    }

    /// Get all watched paths
    pub async fn watched_paths(&self) -> Vec<PathBuf> {
        let paths = self.watched_paths.read().await;
        paths.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_kind_variants() {
        assert_eq!(ChangeKind::Created, ChangeKind::Created);
        assert_eq!(ChangeKind::Modified, ChangeKind::Modified);
        assert_eq!(ChangeKind::Removed, ChangeKind::Removed);
        assert_eq!(ChangeKind::Mixed, ChangeKind::Mixed);
        assert_ne!(ChangeKind::Created, ChangeKind::Modified);
        assert_ne!(ChangeKind::Removed, ChangeKind::Mixed);
    }

    #[test]
    fn test_change_kind_from_notify() {
        use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind};
        use notify::EventKind;
        assert_eq!(ChangeKind::from(&EventKind::Create(CreateKind::File)), ChangeKind::Created);
        assert_eq!(ChangeKind::from(&EventKind::Modify(ModifyKind::Any)), ChangeKind::Modified);
        assert_eq!(ChangeKind::from(&EventKind::Remove(RemoveKind::File)), ChangeKind::Removed);
        assert_eq!(ChangeKind::from(&EventKind::Other), ChangeKind::Mixed);
        assert_eq!(ChangeKind::from(&EventKind::Access(AccessKind::Read)), ChangeKind::Mixed);
    }

    #[test]
    fn test_file_change_debug() {
        let change = FileChange {
            path: PathBuf::from("/test.md"),
            kind: ChangeKind::Modified,
        };
        let debug = format!("{:?}", change);
        assert!(debug.contains("FileChange"));
        assert!(debug.contains("/test.md"));
    }

    #[test]
    fn test_file_change_clone() {
        let change = FileChange {
            path: PathBuf::from("/test.md"),
            kind: ChangeKind::Created,
        };
        let cloned = change.clone();
        assert_eq!(change.path, cloned.path);
        assert_eq!(change.kind, cloned.kind);
    }
}
