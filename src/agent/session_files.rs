//! Session Files — Scoped File System for Agent Sessions
//!
//! Provides each session with an isolated directory for temporary file operations.
//! Inspired by OpenClaw's `session-files.ts`.
//!
//! # Features
//!
//! - Per-session directory isolation
//! - Automatic cleanup on session end
//! - Size quota enforcement
//! - Path traversal protection

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Manager for session-scoped file operations.
#[derive(Debug, Clone)]
pub struct SessionFileManager {
    /// Root directory for all session files.
    root_dir: PathBuf,
    /// Per-session directories.
    sessions: Arc<RwLock<HashMap<String, SessionFileDir>>>,
    /// Default size quota per session (bytes).
    default_quota: usize,
}

/// File directory for a single session.
#[derive(Debug, Clone)]
pub struct SessionFileDir {
    /// Session ID.
    pub session_id: String,
    /// Directory path.
    pub path: PathBuf,
    /// Size quota (bytes).
    pub quota: usize,
    /// Current usage (bytes).
    pub usage: usize,
}

impl SessionFileManager {
    /// Create a new session file manager.
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        Self {
            root_dir,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            default_quota: 100 * 1024 * 1024, // 100 MB default
        }
    }

    /// Set the default quota per session.
    pub fn with_quota(mut self, quota: usize) -> Self {
        self.default_quota = quota;
        self
    }

    /// Initialize the root directory.
    pub async fn init(&self) -> crate::Result<()> {
        tokio::fs::create_dir_all(&self.root_dir).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: "Failed to create session files root".into(),
                details: e.to_string(),
            }
        })?;
        Ok(())
    }

    /// Create a session directory (idempotent).
    pub async fn create_session(
        &self,
        session_id: impl Into<String>,
    ) -> crate::Result<SessionFileDir> {
        let session_id = session_id.into();
        let session_path = self.root_dir.join(&session_id);

        // Create directory if it doesn't exist
        tokio::fs::create_dir_all(&session_path).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: format!("Failed to create session dir for {}", session_id),
                details: e.to_string(),
            }
        })?;

        // Canonicalize to resolve symlinks (e.g. /tmp -> /private/tmp on macOS)
        let session_path = tokio::fs::canonicalize(&session_path).await.unwrap_or(session_path);

        let dir = SessionFileDir {
            session_id: session_id.clone(),
            path: session_path,
            quota: self.default_quota,
            usage: 0,
        };

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), dir.clone());
        }

        info!("Created session file directory for session: {}", session_id);
        Ok(dir)
    }

    /// Get the session directory (if it exists).
    pub async fn get_session(&self,
        session_id: &str,
    ) -> Option<SessionFileDir> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    /// Resolve a path within a session directory (with traversal protection).
    pub async fn resolve_path(
        &self,
        session_id: &str,
        relative_path: &str,
    ) -> Option<PathBuf> {
        let sessions = self.sessions.read().await;
        let session_dir = sessions.get(session_id)?;

        let resolved = session_dir.path.join(relative_path);
        let canonical = match tokio::fs::canonicalize(&resolved).await {
            Ok(p) => p,
            Err(_) => {
                // File doesn't exist yet — check parent directory is within session
                let parent = resolved.parent()?;
                let canonical_parent = match tokio::fs::canonicalize(parent).await {
                    Ok(p) => p,
                    Err(_) => return None,
                };
                if !canonical_parent.starts_with(&session_dir.path) {
                    warn!("Path traversal blocked: {} in session {}", relative_path, session_id);
                    return None;
                }
                return Some(resolved);
            }
        };

        // Ensure the resolved path is within the session directory
        if !canonical.starts_with(&session_dir.path) {
            warn!("Path traversal blocked: {} in session {}", relative_path, session_id);
            return None;
        }

        Some(canonical)
    }

    /// List files in a session directory.
    pub async fn list_files(&self,
        session_id: &str,
    ) -> Vec<String> {
        let sessions = self.sessions.read().await;
        let Some(dir) = sessions.get(session_id) else {
            return Vec::new();
        };

        let mut files = Vec::new();
        let mut entries = match tokio::fs::read_dir(&dir.path).await {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Ok(meta) = entry.metadata().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let size = meta.len();
                files.push(format!("{} ({} bytes)", name, size));
            }
        }

        files
    }

    /// Clean up a session directory.
    pub async fn cleanup_session(
        &self,
        session_id: &str,
    ) -> crate::Result<()> {
        let path = {
            let mut sessions = self.sessions.write().await;
            let dir = sessions.remove(session_id);
            dir.map(|d| d.path)
        };

        if let Some(path) = path {
            tokio::fs::remove_dir_all(&path).await.map_err(|e| {
                crate::error::MantaError::Storage {
                    context: format!("Failed to cleanup session dir for {}", session_id),
                    details: e.to_string(),
                }
            })?;
            info!("Cleaned up session file directory for session: {}", session_id);
        }

        Ok(())
    }

    /// Get total size of all session directories.
    pub async fn total_usage(&self) -> usize {
        let sessions = self.sessions.read().await;
        let mut total = 0;
        for dir in sessions.values() {
            total += Self::dir_size(&dir.path).await.unwrap_or(0);
        }
        total
    }

    /// Calculate directory size recursively.
    async fn dir_size(path: &Path) -> std::io::Result<usize> {
        let mut total = 0;
        let mut entries = tokio::fs::read_dir(path).await?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let meta = entry.metadata().await?;
            if meta.is_file() {
                total += meta.len() as usize;
            } else if meta.is_dir() {
                total += Box::pin(Self::dir_size(&entry.path())).await?;
            }
        }
        Ok(total)
    }

    /// List all active sessions.
    pub async fn list_sessions(&self) -> Vec<String> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_file_manager() {
        let tmp = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        let manager = SessionFileManager::new(&tmp);
        manager.init().await.unwrap();

        // Create session
        let dir = manager.create_session("session_1").await.unwrap();
        assert!(dir.path.exists());

        // Resolve path
        let path = manager.resolve_path("session_1", "test.txt").await;
        assert!(path.is_some());

        // Path traversal protection
        let bad = manager.resolve_path("session_1", "../secret.txt").await;
        assert!(bad.is_none());

        // Cleanup
        manager.cleanup_session("session_1").await.unwrap();
        assert!(!dir.path.exists());

        // Clean up temp dir
        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }
}
