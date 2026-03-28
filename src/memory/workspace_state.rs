//! Workspace State Tracking for Manta
//!
//! Tracks initialization progress and setup state for OpenClaw-style workspaces.
//! Similar to OpenClaw's .openclaw/workspace-state.json
//!
//! State file location: ~/.manta/workspace/.manta/workspace-state.json
//!
//! ## State Fields
//!
//! - `version`: State file format version (currently 1)
//! - `bootstrap_seeded_at`: ISO 8601 timestamp when BOOTSTRAP.md was first created
//! - `setup_completed_at`: ISO 8601 timestamp when setup completed (BOOTSTRAP.md deleted)
//!
//! ## State Transitions
//!
//! 1. **Brand new workspace**: No state file exists
//! 2. **Bootstrap seeded**: `bootstrap_seeded_at` is set when BOOTSTRAP.md is created
//! 3. **Setup completed**: `setup_completed_at` is set when BOOTSTRAP.md is deleted after user completes onboarding
//!
//! ## Workspace Detection
//!
//! A workspace is considered "existing" (not brand new) if any of:
//! - State file has `setup_completed_at`
//! - State file has `bootstrap_seeded_at` AND BOOTSTRAP.md still exists
//! - User content indicators exist (MEMORY.md, memory/ dir, .git/ dir)
//! - Core template files have been customized (IDENTITY.md or USER.md differ from templates)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::fs;
use tracing::{debug, info, warn};

/// Current state file format version
pub const WORKSPACE_STATE_VERSION: u32 = 1;

/// Workspace state tracked in .manta/workspace/.manta/workspace-state.json
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceState {
    /// State file format version
    pub version: u32,

    /// ISO 8601 timestamp when BOOTSTRAP.md was first seeded
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap_seeded_at: Option<String>,

    /// ISO 8601 timestamp when setup was completed
    #[serde(
        skip_serializing_if = "Option::is_none",
        alias = "onboarding_completed_at"
    )]
    pub setup_completed_at: Option<String>,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            version: WORKSPACE_STATE_VERSION,
            bootstrap_seeded_at: None,
            setup_completed_at: None,
        }
    }
}

impl WorkspaceState {
    /// Create a new default state
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if setup has been completed
    pub fn is_setup_completed(&self) -> bool {
        self.setup_completed_at
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }

    /// Check if bootstrap has been seeded
    pub fn is_bootstrap_seeded(&self) -> bool {
        self.bootstrap_seeded_at
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }

    /// Mark bootstrap as seeded
    pub fn mark_bootstrap_seeded(&mut self) {
        self.bootstrap_seeded_at = Some(SystemTime::now().to_iso8601());
    }

    /// Mark setup as completed
    pub fn mark_setup_completed(&mut self) {
        self.setup_completed_at = Some(SystemTime::now().to_iso8601());
    }

    /// Load state from file, returning default if not exists or invalid
    pub async fn load(state_path: &Path) -> crate::Result<Self> {
        match fs::read_to_string(state_path).await {
            Ok(raw) => match Self::parse(&raw) {
                Some(state) => {
                    // Migrate legacy onboarding_completed_at to setup_completed_at
                    if raw.contains("\"onboarding_completed_at\"")
                        && !raw.contains("\"setup_completed_at\"")
                        && state.setup_completed_at.is_some()
                    {
                        // State was already migrated during parsing
                    }
                    Ok(state)
                }
                None => {
                    warn!("Invalid workspace state format, using defaults");
                    Ok(Self::default())
                }
            },
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    debug!("No workspace state file found, using defaults");
                    Ok(Self::default())
                } else {
                    Err(crate::error::MantaError::Storage {
                        context: format!("Failed to read workspace state: {:?}", state_path),
                        details: e.to_string(),
                    })
                }
            }
        }
    }

    /// Parse state from JSON string
    fn parse(raw: &str) -> Option<Self> {
        serde_json::from_str::<Self>(raw).ok()
    }

    /// Save state to file atomically
    pub async fn save(&self, state_path: &Path) -> crate::Result<()> {
        let parent = state_path
            .parent()
            .ok_or_else(|| crate::error::MantaError::Storage {
                context: "Invalid state file path".to_string(),
                details: "Path has no parent directory".to_string(),
            })?;

        fs::create_dir_all(parent)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: format!("Failed to create state directory: {:?}", parent),
                details: e.to_string(),
            })?;

        let payload =
            serde_json::to_string_pretty(self).map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to serialize workspace state".to_string(),
                details: e.to_string(),
            })?;

        // Write atomically via temp file + rename
        let tmp_path = state_path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            std::time::UNIX_EPOCH
                .elapsed()
                .unwrap_or_default()
                .as_millis()
        ));

        match fs::write(&tmp_path, format!("{}\n", payload)).await {
            Ok(()) => {
                fs::rename(&tmp_path, state_path).await.map_err(|e| {
                    crate::error::MantaError::Storage {
                        context: format!("Failed to rename state file: {:?}", tmp_path),
                        details: e.to_string(),
                    }
                })?;
                Ok(())
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp_path).await;
                Err(crate::error::MantaError::Storage {
                    context: format!("Failed to write state file: {:?}", tmp_path),
                    details: e.to_string(),
                })
            }
        }
    }
}

/// Extension trait for SystemTime to format as ISO 8601
trait SystemTimeExt {
    fn to_iso8601(&self) -> String;
}

impl SystemTimeExt for SystemTime {
    fn to_iso8601(&self) -> String {
        // Use RFC 3339 which is ISO 8601 compatible
        self.duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| {
                chrono::DateTime::<chrono::Utc>::from_timestamp(
                    d.as_secs() as i64,
                    d.subsec_nanos(),
                )
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339())
            })
            .unwrap_or_else(|_| chrono::Utc::now().to_rfc3339())
    }
}

/// Workspace manager for state tracking and initialization
#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    workspace_dir: PathBuf,
    state_path: PathBuf,
}

impl WorkspaceManager {
    /// Create a new workspace manager for the given directory
    pub fn new(workspace_dir: PathBuf) -> Self {
        let state_path = workspace_dir.join(".manta").join("workspace-state.json");
        Self { workspace_dir, state_path }
    }

    /// Load the current workspace state
    pub async fn load_state(&self) -> crate::Result<WorkspaceState> {
        WorkspaceState::load(&self.state_path).await
    }

    /// Save workspace state
    pub async fn save_state(&self, state: &WorkspaceState) -> crate::Result<()> {
        state.save(&self.state_path).await
    }

    /// Check if this is a brand new workspace (never initialized)
    ///
    /// A workspace is considered "brand new" if:
    /// - No state file exists, AND
    /// - No user content indicators exist (memory/, MEMORY.md, .git/)
    pub async fn is_brand_new(&self) -> bool {
        // If state file exists and has setup_completed_at, not brand new
        if let Ok(state) = self.load_state().await {
            if state.is_setup_completed() {
                return false;
            }
        }

        // Check for user content indicators
        self.check_user_content_indicators().await.is_none()
    }

    /// Check for user content indicators that suggest an existing workspace
    ///
    /// Returns Some(indicator_name) if found, None if workspace appears empty
    async fn check_user_content_indicators(&self) -> Option<String> {
        // Check for memory directory or files
        let memory_dir = self.workspace_dir.join("memory");
        if memory_dir.exists() {
            return Some("memory/".to_string());
        }

        let memory_md = self.workspace_dir.join("MEMORY.md");
        if memory_md.exists() {
            return Some("MEMORY.md".to_string());
        }

        let memory_lower_md = self.workspace_dir.join("memory.md");
        if memory_lower_md.exists() {
            return Some("memory.md".to_string());
        }

        // Check for .git directory
        let git_dir = self.workspace_dir.join(".git");
        if git_dir.exists() {
            return Some(".git/".to_string());
        }

        None
    }

    /// Check if workspace setup is completed
    pub async fn is_setup_completed(&self) -> bool {
        self.load_state()
            .await
            .map(|s| s.is_setup_completed())
            .unwrap_or(false)
    }

    /// Check if bootstrap has been seeded
    pub async fn is_bootstrap_seeded(&self) -> bool {
        self.load_state()
            .await
            .map(|s| s.is_bootstrap_seeded())
            .unwrap_or(false)
    }

    /// Mark bootstrap as seeded and save state
    pub async fn mark_bootstrap_seeded(&self) -> crate::Result<()> {
        let mut state = self.load_state().await?;
        state.mark_bootstrap_seeded();
        self.save_state(&state).await?;
        info!("Marked bootstrap as seeded in workspace state");
        Ok(())
    }

    /// Mark setup as completed and save state
    pub async fn mark_setup_completed(&self) -> crate::Result<()> {
        let mut state = self.load_state().await?;
        state.mark_setup_completed();
        self.save_state(&state).await?;
        info!("Marked setup as completed in workspace state");
        Ok(())
    }

    /// Initialize git repo if not already present
    ///
    /// Only initializes for brand new workspaces (won't touch existing repos)
    pub async fn ensure_git_repo(&self, is_brand_new: bool) -> bool {
        if !is_brand_new {
            debug!("Skipping git init - workspace is not brand new");
            return false;
        }

        let git_dir = self.workspace_dir.join(".git");
        if git_dir.exists() {
            debug!("Git repo already exists");
            return true;
        }

        // Check if git is available
        if !is_git_available().await {
            debug!("Git not available, skipping repo initialization");
            return false;
        }

        // Initialize git repo
        match tokio::process::Command::new("git")
            .arg("init")
            .current_dir(&self.workspace_dir)
            .output()
            .await
        {
            Ok(output) => {
                if output.status.success() {
                    info!("Initialized git repo in workspace: {:?}", self.workspace_dir);
                    true
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    debug!("Git init failed: {}", stderr);
                    false
                }
            }
            Err(e) => {
                debug!("Failed to run git init: {}", e);
                false
            }
        }
    }

    /// Get the workspace directory path
    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    /// Get the state file path
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// Check if bootstrap file should be created
    ///
    /// Returns true if:
    /// - Setup is not completed AND
    /// - (Workspace is brand new OR bootstrap was seeded but file is missing)
    pub async fn should_create_bootstrap(&self) -> bool {
        // If setup is completed, never create bootstrap
        if self.is_setup_completed().await {
            return false;
        }

        // If brand new workspace, create bootstrap
        if self.is_brand_new().await {
            return true;
        }

        // If bootstrap was seeded but file is missing (partial init recovery)
        if self.is_bootstrap_seeded().await {
            let bootstrap_path = self.workspace_dir.join("BOOTSTRAP.md");
            return !bootstrap_path.exists();
        }

        false
    }

    /// Handle bootstrap file deletion (user completed onboarding)
    ///
    /// Call this when BOOTSTRAP.md is deleted to mark setup as completed
    pub async fn on_bootstrap_deleted(&self) -> crate::Result<()> {
        self.mark_setup_completed().await
    }
}

/// Check if git is available on the system
async fn is_git_available() -> bool {
    tokio::process::Command::new("git")
        .arg("--version")
        .output()
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_workspace() -> (TempDir, WorkspaceManager) {
        let temp_dir = TempDir::new().unwrap();
        let manager = WorkspaceManager::new(temp_dir.path().to_path_buf());
        (temp_dir, manager)
    }

    #[tokio::test]
    async fn test_workspace_state_default() {
        let state = WorkspaceState::default();
        assert_eq!(state.version, WORKSPACE_STATE_VERSION);
        assert!(!state.is_setup_completed());
        assert!(!state.is_bootstrap_seeded());
    }

    #[tokio::test]
    async fn test_workspace_state_serialization() {
        let mut state = WorkspaceState::new();
        state.mark_bootstrap_seeded();
        state.mark_setup_completed();

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"version\""));
        assert!(json.contains("\"bootstrap_seeded_at\""));
        assert!(json.contains("\"setup_completed_at\""));

        let parsed: WorkspaceState = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_setup_completed());
        assert!(parsed.is_bootstrap_seeded());
    }

    #[tokio::test]
    async fn test_workspace_state_load_save() {
        let (_temp_dir, manager) = create_test_workspace();

        let mut state = WorkspaceState::new();
        state.mark_bootstrap_seeded();
        manager.save_state(&state).await.unwrap();

        let loaded = manager.load_state().await.unwrap();
        assert!(loaded.is_bootstrap_seeded());
        assert!(!loaded.is_setup_completed());
    }

    #[tokio::test]
    async fn test_workspace_state_legacy_migration() {
        let (_temp_dir, manager) = create_test_workspace();

        // Write legacy format with onboarding_completed_at
        let legacy_json = r#"{
  "version": 1,
  "onboarding_completed_at": "2026-03-15T02:30:00.000Z"
}"#;

        fs::create_dir_all(manager.state_path.parent().unwrap())
            .await
            .unwrap();
        fs::write(&manager.state_path, legacy_json).await.unwrap();

        let state = manager.load_state().await.unwrap();
        assert!(state.is_setup_completed());
        assert_eq!(state.setup_completed_at, Some("2026-03-15T02:30:00.000Z".to_string()));
    }

    #[tokio::test]
    async fn test_is_brand_new_empty_workspace() {
        let (_temp_dir, manager) = create_test_workspace();
        assert!(manager.is_brand_new().await);
    }

    #[tokio::test]
    async fn test_is_brand_new_with_memory_dir() {
        let (_temp_dir, manager) = create_test_workspace();
        fs::create_dir_all(manager.workspace_dir.join("memory"))
            .await
            .unwrap();
        assert!(!manager.is_brand_new().await);
    }

    #[tokio::test]
    async fn test_is_brand_new_with_git_dir() {
        let (_temp_dir, manager) = create_test_workspace();
        fs::create_dir_all(manager.workspace_dir.join(".git"))
            .await
            .unwrap();
        assert!(!manager.is_brand_new().await);
    }

    #[tokio::test]
    async fn test_is_brand_new_with_state_completed() {
        let (_temp_dir, manager) = create_test_workspace();
        let mut state = WorkspaceState::new();
        state.mark_setup_completed();
        manager.save_state(&state).await.unwrap();
        assert!(!manager.is_brand_new().await);
    }
}
