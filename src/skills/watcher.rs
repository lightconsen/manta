//! File watcher for hot reloading skills
//!
//! Watches skill directories for changes and triggers reloads.
//! Uses the `notify` crate for cross-platform file system events.

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    /// Callback for file changes (stored for future use)
    _callback: Arc<RwLock<Option<FileChangeCallback>>>,
}

impl SkillWatcher {
    /// Create a new skill watcher with paths and callback
    pub fn new<F>(paths: Vec<(StorageLevel, PathBuf)>, callback: F) -> crate::Result<Self>
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::unbounded_channel();
        let watched_paths = Arc::new(RwLock::new(HashSet::new()));

        let tx_clone = tx.clone();
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
                            callback(path.to_string_lossy().to_string());
                        }
                    }
                }
                Err(e) => {
                    error!("File watcher error: {}", e);
                }
            }
        })
        .map_err(|e| {
            crate::error::MantaError::Internal(format!("Failed to create file watcher: {}", e))
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
            _callback: Arc::new(RwLock::new(None)),
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
        let mut new_watcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    let kind: ChangeKind = (&event.kind).into();
                    for path in event.paths {
                        if path.to_string_lossy().ends_with("SKILL.md") {
                            if let Err(e) = tx_clone.send(FileChange { path, kind }) {
                                warn!("Failed to send file change event: {}", e);
                            }
                        }
                    }
                }
                Err(e) => error!("File watcher error: {}", e),
            })
            .map_err(|e| {
                crate::error::MantaError::Internal(format!(
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

/// Simple polling-based watcher as fallback
#[allow(dead_code)]
pub struct PollingWatcher {
    /// Polling interval
    interval: std::time::Duration,
    /// Last known state of watched files
    state: Arc<RwLock<HashSet<PathBuf>>>,
    /// Channel for change events
    rx: mpsc::UnboundedReceiver<FileChange>,
    /// Shutdown signal
    _shutdown_tx: tokio::sync::oneshot::Sender<()>,
}

#[allow(dead_code)]
impl PollingWatcher {
    /// Create a new polling watcher
    pub fn new(interval: std::time::Duration) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

        let state: Arc<RwLock<HashSet<PathBuf>>> = Arc::new(RwLock::new(HashSet::new()));
        let state_clone = Arc::clone(&state);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        Self::check_changes(&state_clone, &tx).await;
                    }
                    _ = &mut shutdown_rx => {
                        debug!("Polling watcher shutting down");
                        break;
                    }
                }
            }
        });

        Self {
            interval,
            state,
            rx,
            _shutdown_tx: shutdown_tx,
        }
    }

    /// Watch a directory
    pub async fn watch_dir(&self, path: &Path) -> crate::Result<()> {
        let mut state = self.state.write().await;

        // Add all files in the directory
        if let Ok(mut entries) = tokio::fs::read_dir(path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_file() {
                    state.insert(path);
                }
            }
        }

        info!("Polling watcher now tracking {} files", state.len());
        Ok(())
    }

    /// Check for changes
    async fn check_changes(
        _state: &Arc<RwLock<HashSet<PathBuf>>>,
        _tx: &mpsc::UnboundedSender<FileChange>,
    ) {
        // Simplified implementation - in a real one, we'd compare
        // file metadata to detect changes
    }

    /// Get the next file change event
    pub async fn recv(&mut self) -> Option<FileChange> {
        self.rx.recv().await
    }
}

/// Debounced change events
#[allow(dead_code)]
pub struct DebouncedWatcher {
    /// Inner watcher
    watcher: SkillWatcher,
    /// Debounce duration
    debounce: std::time::Duration,
    /// Pending changes
    pending: Arc<RwLock<Vec<FileChange>>>,
}

#[allow(dead_code)]
impl DebouncedWatcher {
    /// Create a new debounced watcher
    pub fn new<F>(
        paths: Vec<(StorageLevel, PathBuf)>,
        debounce: std::time::Duration,
        callback: F,
    ) -> crate::Result<Self>
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        let watcher = SkillWatcher::new(paths, callback)?;

        Ok(Self {
            watcher,
            debounce,
            pending: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Watch a directory
    pub async fn watch_dir(&mut self, path: &Path) -> crate::Result<()> {
        self.watcher.watch_dir(path).await
    }

    /// Get debounced changes
    pub async fn recv_debounced(&mut self) -> Option<Vec<FileChange>> {
        // Collect all changes within the debounce window
        let start = tokio::time::Instant::now();
        let mut changes = Vec::new();

        while start.elapsed() < self.debounce {
            match tokio::time::timeout(std::time::Duration::from_millis(50), self.watcher.recv())
                .await
            {
                Ok(Some(change)) => {
                    changes.push(change);
                }
                Ok(None) => break,
                Err(_) => continue, // Timeout, continue collecting
            }
        }

        if changes.is_empty() {
            None
        } else {
            Some(changes)
        }
    }
}

/// Check if a file change is relevant for skill reloading
#[allow(dead_code)]
pub fn is_skill_file_change(change: &FileChange) -> bool {
    let path_str = change.path.to_string_lossy();

    // Only care about SKILL.md files
    if path_str.ends_with("SKILL.md") {
        return true;
    }

    // Or files in skill directories
    if path_str.contains("/skills/") || path_str.contains("\\skills\\") {
        return true;
    }

    false
}

/// Get the skill name from a file change path
#[allow(dead_code)]
pub fn skill_name_from_change(change: &FileChange) -> Option<String> {
    let path = &change.path;

    // If it's SKILL.md, get parent directory name
    if path.file_name()?.to_str()? == "SKILL.md" {
        return path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string());
    }

    // Otherwise, try to extract from path containing /skills/
    let path_str = path.to_string_lossy();
    if let Some(pos) = path_str.find("/skills/") {
        let after_skills = &path_str[pos + 8..];
        if let Some(slash_pos) = after_skills.find('/') {
            return Some(after_skills[..slash_pos].to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_kind_variants() {
        // Test that ChangeKind variants exist and can be compared
        assert_eq!(ChangeKind::Created, ChangeKind::Created);
        assert_eq!(ChangeKind::Modified, ChangeKind::Modified);
        assert_eq!(ChangeKind::Removed, ChangeKind::Removed);
        assert_eq!(ChangeKind::Mixed, ChangeKind::Mixed);
        assert_ne!(ChangeKind::Created, ChangeKind::Modified);
    }

    #[test]
    fn test_is_skill_file_change() {
        let change = FileChange {
            path: PathBuf::from("/skills/docker/SKILL.md"),
            kind: ChangeKind::Modified,
        };
        assert!(is_skill_file_change(&change));

        let change = FileChange {
            path: PathBuf::from("/other/file.txt"),
            kind: ChangeKind::Modified,
        };
        assert!(!is_skill_file_change(&change));
    }

    #[test]
    fn test_skill_name_from_change() {
        let change = FileChange {
            path: PathBuf::from("/skills/docker/SKILL.md"),
            kind: ChangeKind::Modified,
        };
        assert_eq!(skill_name_from_change(&change), Some("docker".to_string()));

        let change = FileChange {
            path: PathBuf::from("/skills/k8s/README.md"),
            kind: ChangeKind::Modified,
        };
        assert_eq!(skill_name_from_change(&change), Some("k8s".to_string()));
    }
}
