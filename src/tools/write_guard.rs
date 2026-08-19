//! Read-before-edit observation store for file-writing tools.
//!
//! A `WriteGuard` tracks, per conversation, which file versions the model
//! has actually seen. `file_read` records an observation after a successful
//! read; `file_write` / `file_edit` consult the guard before modifying an
//! existing file and reject with corrective feedback when the file was never
//! read or has changed since the last observation. Writes and edits record
//! the new version afterwards, so read → edit → edit flows work without
//! re-reading.
//!
//! Inspired by the fs observation-policy seam in DeepSeek Harness: an
//! unobserved write is the model acting on stale assumptions; a stale write
//! silently clobbers someone else's (or another agent's) concurrent edit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A filesystem-state fingerprint used to detect changes since observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileVersion {
    mtime_nanos: u128,
    len: u64,
}

impl FileVersion {
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime_nanos = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self { mtime_nanos, len: meta.len() })
    }
}

/// Why a write was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteGuardError {
    /// The file exists but was never read in this conversation.
    MustReadFirst,
    /// The file changed on disk since the last observation in this
    /// conversation.
    StaleSinceRead,
}

/// Per-conversation observation table: (conversation_id, path) → version.
#[derive(Debug, Default)]
pub struct WriteGuard {
    observed: std::sync::RwLock<HashMap<(String, PathBuf), FileVersion>>,
}

impl WriteGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `conversation_id` has seen `path` at its current version.
    /// No-op when the file does not exist (nothing to observe).
    pub fn record(&self, conversation_id: &str, path: &Path) {
        let Some(version) = FileVersion::of(path) else {
            return;
        };
        if let Ok(mut map) = self.observed.write() {
            map.insert((conversation_id.to_string(), path.to_path_buf()), version);
        }
    }

    /// Whether `path` may be written by `conversation_id`.
    ///
    /// Non-existent targets are always writable (creation needs no prior
    /// observation). Existing files must have a fresh observation.
    pub fn check(&self, conversation_id: &str, path: &Path) -> Result<(), WriteGuardError> {
        let Some(current) = FileVersion::of(path) else {
            return Ok(()); // target does not exist — creation is allowed
        };
        let observed = self.observed.read().ok().and_then(|map| {
            map.get(&(conversation_id.to_string(), path.to_path_buf()))
                .copied()
        });
        match observed {
            None => Err(WriteGuardError::MustReadFirst),
            Some(v) if v != current => Err(WriteGuardError::StaleSinceRead),
            Some(_) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("syscity_wg_{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn creation_allowed_without_observation() {
        let guard = WriteGuard::new();
        let missing = std::env::temp_dir().join(format!("syscity_wg_{}.txt", uuid::Uuid::new_v4()));
        assert!(guard.check("conv1", &missing).is_ok());
    }

    #[test]
    fn existing_file_requires_read_first() {
        let guard = WriteGuard::new();
        let path = tmp_file("hello");
        assert_eq!(guard.check("conv1", &path), Err(WriteGuardError::MustReadFirst));
        guard.record("conv1", &path);
        assert!(guard.check("conv1", &path).is_ok());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn stale_after_external_modification() {
        let guard = WriteGuard::new();
        let path = tmp_file("v1");
        guard.record("conv1", &path);
        // External change: different content AND a forced-newer mtime.
        std::fs::write(&path, "v2-longer").unwrap();
        assert_eq!(guard.check("conv1", &path), Err(WriteGuardError::StaleSinceRead));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn write_then_edit_flows_without_reread() {
        let guard = WriteGuard::new();
        let path = tmp_file("v1");
        guard.record("conv1", &path); // write records the new version
        std::fs::write(&path, "v2").unwrap();
        guard.record("conv1", &path); // edit records again
        assert!(guard.check("conv1", &path).is_ok());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn observations_are_per_conversation() {
        let guard = WriteGuard::new();
        let path = tmp_file("hello");
        guard.record("conv1", &path);
        assert_eq!(guard.check("conv2", &path), Err(WriteGuardError::MustReadFirst));
        std::fs::remove_file(&path).ok();
    }
}
