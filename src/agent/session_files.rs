//! Session Files — Scoped File System for Agent Sessions
//!
//! Provides each session with an isolated directory for temporary file operations.
//! ts`.
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
        tokio::fs::create_dir_all(&self.root_dir)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to create session files root".into(),
                details: e.to_string(),
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
        tokio::fs::create_dir_all(&session_path)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to create session dir for {}", session_id),
                details: e.to_string(),
            })?;

 // Canonicalize to resolve symlinks (e.g. /tmp -> /private/tmp on macOS)
        let session_path = tokio::fs::canonicalize(&session_path)
            .await
            .unwrap_or(session_path);

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
    pub async fn get_session(&self, session_id: &str) -> Option<SessionFileDir> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

 /// Resolve a path within a session directory (with traversal protection).
    pub async fn resolve_path(&self, session_id: &str, relative_path: &str) -> Option<PathBuf> {
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
    pub async fn list_files(&self, session_id: &str) -> Vec<String> {
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
    pub async fn cleanup_session(&self, session_id: &str) -> crate::Result<()> {
        let path = {
            let mut sessions = self.sessions.write().await;
            let dir = sessions.remove(session_id);
            dir.map(|d| d.path)
        };

        if let Some(path) = path {
            tokio::fs::remove_dir_all(&path).await.map_err(|e| {
                crate::error::SyscityError::Storage {
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
        let tmp = std::env::temp_dir().join(format!("syscity_test_{}", uuid::Uuid::new_v4()));
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

    #[test]
    fn test_session_file_manager_new() {
        let manager = SessionFileManager::new("/tmp/test_sessions");
        assert_eq!(manager.default_quota, 100 * 1024 * 1024);
    }

    #[test]
    fn test_session_file_manager_with_quota() {
        let manager = SessionFileManager::new("/tmp/test_sessions").with_quota(50 * 1024 * 1024);
        assert_eq!(manager.default_quota, 50 * 1024 * 1024);
    }

    #[tokio::test]
    async fn test_init_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();
        manager.init().await.unwrap(); // Should not fail
        assert!(tmp.path().exists());
    }

    #[tokio::test]
    async fn test_create_session_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();

        let dir1 = manager.create_session("s1").await.unwrap();
        let dir2 = manager.create_session("s1").await.unwrap();
        assert_eq!(dir1.path, dir2.path);
        assert!(dir1.path.exists());
    }

    #[tokio::test]
    async fn test_get_session() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();

        assert!(manager.get_session("nonexistent").await.is_none());

        manager.create_session("s1").await.unwrap();
        let session = manager.get_session("s1").await;
        assert!(session.is_some());
        assert_eq!(session.unwrap().session_id, "s1");
    }

    #[tokio::test]
    async fn test_resolve_path_for_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();
        manager.create_session("s1").await.unwrap();

 // New file in existing parent (session root)
        let path = manager.resolve_path("s1", "file.txt").await;
        assert!(path.is_some());
    }

    #[tokio::test]
    async fn test_resolve_path_for_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();
        manager.create_session("s1").await.unwrap();

        let path = manager.resolve_path("s1", "test.txt").await.unwrap();
        tokio::fs::write(&path, "hello").await.unwrap();

        let resolved = manager.resolve_path("s1", "test.txt").await;
        assert!(resolved.is_some());
    }

    #[tokio::test]
    async fn test_resolve_path_traversal_nested() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();
        manager.create_session("s1").await.unwrap();

 // Nested traversal
        let bad = manager.resolve_path("s1", "foo/../../secret.txt").await;
        assert!(bad.is_none());
    }

    #[tokio::test]
    async fn test_resolve_path_missing_session() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();

        let path = manager.resolve_path("nonexistent", "file.txt").await;
        assert!(path.is_none());
    }

    #[tokio::test]
    async fn test_list_files() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();
        manager.create_session("s1").await.unwrap();

        let path = manager.resolve_path("s1", "a.txt").await.unwrap();
        tokio::fs::write(&path, "hello").await.unwrap();

        let path = manager.resolve_path("s1", "b.txt").await.unwrap();
        tokio::fs::write(&path, "world").await.unwrap();

        let files = manager.list_files("s1").await;
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.contains("a.txt")));
        assert!(files.iter().any(|f| f.contains("b.txt")));
    }

    #[tokio::test]
    async fn test_list_files_missing_session() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        let files = manager.list_files("nonexistent").await;
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_session_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();
 // Should not panic
        manager.cleanup_session("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn test_total_usage() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();
        manager.create_session("s1").await.unwrap();

        let path = manager.resolve_path("s1", "file.txt").await.unwrap();
        tokio::fs::write(&path, "hello world").await.unwrap();

        let usage = manager.total_usage().await;
        assert_eq!(usage, 11); // "hello world" is 11 bytes
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = SessionFileManager::new(tmp.path());
        manager.init().await.unwrap();

        assert!(manager.list_sessions().await.is_empty());

        manager.create_session("s1").await.unwrap();
        manager.create_session("s2").await.unwrap();

        let sessions = manager.list_sessions().await;
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&"s1".to_string()));
        assert!(sessions.contains(&"s2".to_string()));
    }

    #[test]
    fn test_session_file_dir_clone() {
        let dir = SessionFileDir {
            session_id: "s1".to_string(),
            path: PathBuf::from("/tmp/s1"),
            quota: 100,
            usage: 10,
        };
        let cloned = dir.clone();
        assert_eq!(cloned.session_id, "s1");
        assert_eq!(cloned.quota, 100);
        assert_eq!(cloned.usage, 10);
    }

    #[tokio::test]
    async fn test_dir_size() {
        let tmp = tempfile::tempdir().unwrap();
        tokio::fs::write(tmp.path().join("a.txt"), "hello")
            .await
            .unwrap();
        tokio::fs::create_dir(tmp.path().join("sub")).await.unwrap();
        tokio::fs::write(tmp.path().join("sub/b.txt"), "world")
            .await
            .unwrap();

        let size = SessionFileManager::dir_size(tmp.path()).await.unwrap();
        assert_eq!(size, 10); // "hello" + "world" = 10 bytes
    }
}
