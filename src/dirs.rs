//! Centralized directory management for Manta
//!
//! All Manta data is stored in ~/.manta/ with the following structure:
//! ~/.manta/
//! ├── manta.toml       # Configuration file
//! ├── data/            # SQLite database (manta.db) - unified storage
//! ├── logs/            # Log files (daemon.log)
//! ├── skills/          # User-installed skills
//! ├── agents/          # Agent configurations
//! ├── cron/            # Cron job data
//! ├── todos/           # Task persistence
//! ├── workspace/       # Workspace-level data (SOUL.md, IDENTITY.md, BOOTSTRAP.md, USER.md)
//! └── memory/          # Legacy directory (deprecated, kept for backward compatibility)

use std::path::PathBuf;
use tracing::{debug, info};

/// Base directory name
const MANTA_DIR: &str = ".manta";

/// Get the home directory
fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Get the base Manta directory (~/.manta)
pub fn manta_dir() -> PathBuf {
    home_dir()
        .map(|h| h.join(MANTA_DIR))
        .unwrap_or_else(|| PathBuf::from(MANTA_DIR))
}

/// Get the config directory (~/.manta)
pub fn config_dir() -> PathBuf {
    manta_dir()
}

/// Get the memory/database directory (~/.manta/memory)
pub fn memory_dir() -> PathBuf {
    manta_dir().join("memory")
}

/// Get the workspace data directory for OpenClaw-style files (~/.manta/workspace)
///
/// This is where SOUL.md, IDENTITY.md, BOOTSTRAP.md, and USER.md are stored.
pub fn workspace_memory_dir() -> PathBuf {
    workspace_data_dir()
}

/// Deprecated: Use workspace_memory_dir() instead
#[deprecated(since = "0.1.0", note = "Use workspace_memory_dir() instead")]
pub fn memory_files_dir() -> PathBuf {
    workspace_data_dir()
}

/// Get the logs directory (~/.manta/logs)
pub fn logs_dir() -> PathBuf {
    manta_dir().join("logs")
}

/// Get the skills directory (~/.manta/skills)
pub fn skills_dir() -> PathBuf {
    manta_dir().join("skills")
}

/// Get the agents directory (~/.manta/agents)
pub fn agents_dir() -> PathBuf {
    manta_dir().join("agents")
}

/// Get the cron directory (~/.manta/cron)
pub fn cron_dir() -> PathBuf {
    manta_dir().join("cron")
}

/// Get the workspace data directory (~/.manta/workspace)
pub fn workspace_data_dir() -> PathBuf {
    manta_dir().join("workspace")
}

/// Get the data directory (~/.manta/data)
pub fn data_dir() -> PathBuf {
    manta_dir().join("data")
}

/// Get the todos directory (~/.manta/todos)
pub fn todos_dir() -> PathBuf {
    manta_dir().join("todos")
}
pub fn teams_dir() -> PathBuf {
    manta_dir().join("teams")
}

/// Get the extensions directory (~/.manta/extensions)
pub fn extensions_dir() -> PathBuf {
    manta_dir().join("extensions")
}

/// Get the transcripts directory (~/.manta/transcripts)
pub fn transcripts_dir() -> PathBuf {
    manta_dir().join("transcripts")
}

/// Get the artifacts directory (~/.manta/artifacts)
pub fn artifacts_dir() -> PathBuf {
    manta_dir().join("artifacts")
}

/// Get the disk budget tracking directory (~/.manta/budget)
pub fn budget_dir() -> PathBuf {
    manta_dir().join("budget")
}

/// Get the session files directory (~/.manta/session_files)
pub fn session_files_dir() -> PathBuf {
    manta_dir().join("session_files")
}

/// Get the group sessions directory (~/.manta/groups)
pub fn groups_dir() -> PathBuf {
    manta_dir().join("groups")
}

/// Get the PID file path (~/.manta/daemon.pid)
pub fn pid_file() -> PathBuf {
    manta_dir().join("daemon.pid")
}

/// Get the default config file path (~/.manta/manta.toml)
pub fn default_config_file() -> PathBuf {
    config_dir().join("manta.toml")
}

/// Get the default memory DB path (~/.manta/data/manta.db)
///
/// Note: Previously returned ~/.manta/memory/memory.db, now consolidated
/// to use the main gateway database for unified storage.
pub fn default_memory_db() -> PathBuf {
    data_dir().join("manta.db")
}

/// Get the default log file path (~/.manta/logs/daemon.log)
pub fn default_log_file() -> PathBuf {
    logs_dir().join("daemon.log")
}

/// Get the workspace state file path (~/.manta/workspace/.manta/workspace-state.json)
pub fn workspace_state_file() -> PathBuf {
    workspace_data_dir()
        .join(".manta")
        .join("workspace-state.json")
}

/// Initialize all Manta directories
///
/// Creates the ~/.manta directory structure if it doesn't exist.
/// Returns the base directory path.
pub async fn init() -> crate::Result<PathBuf> {
    let base = manta_dir();

    // Create all subdirectories
    let dirs = [
        &base,
        &memory_dir(),
        &data_dir(),
        &workspace_data_dir(),
        &logs_dir(),
        &skills_dir(),
        &agents_dir(),
        &cron_dir(),
        &workspace_data_dir(),
        &todos_dir(),
        &transcripts_dir(),
        &artifacts_dir(),
        &budget_dir(),
        &groups_dir(),
    ];

    for dir in &dirs {
        if !dir.exists() {
            debug!("Creating directory: {:?}", dir);
            tokio::fs::create_dir_all(dir).await.map_err(|e| {
                crate::error::MantaError::Storage {
                    context: format!("Failed to create directory: {:?}", dir),
                    details: e.to_string(),
                }
            })?;
        }
    }

    info!("Manta directories initialized at: {:?}", base);
    Ok(base)
}

/// Initialize directories synchronously (for non-async contexts)
pub fn init_sync() -> crate::Result<PathBuf> {
    let base = manta_dir();

    // Create all subdirectories
    let dirs = [
        &base,
        &memory_dir(),
        &data_dir(),
        &workspace_data_dir(),
        &logs_dir(),
        &skills_dir(),
        &agents_dir(),
        &cron_dir(),
        &workspace_data_dir(),
        &todos_dir(),
        &transcripts_dir(),
        &artifacts_dir(),
        &budget_dir(),
        &groups_dir(),
    ];

    for dir in &dirs {
        if !dir.exists() {
            debug!("Creating directory: {:?}", dir);
            std::fs::create_dir_all(dir).map_err(|e| crate::error::MantaError::Storage {
                context: format!("Failed to create directory: {:?}", dir),
                details: e.to_string(),
            })?;
        }
    }

    info!("Manta directories initialized at: {:?}", base);
    Ok(base)
}

/// Check if Manta directories are initialized
pub fn is_initialized() -> bool {
    manta_dir().exists()
}

/// Get the path for a specific file type
pub fn path_for(file_type: FileType) -> PathBuf {
    match file_type {
        FileType::Config => default_config_file(),
        FileType::MemoryDb => default_memory_db(),
        FileType::Log => default_log_file(),
        FileType::Pid => pid_file(),
        FileType::Soul => workspace_data_dir().join("SOUL.md"),
        FileType::Identity => workspace_data_dir().join("IDENTITY.md"),
        FileType::Bootstrap => workspace_data_dir().join("BOOTSTRAP.md"),
        FileType::User => workspace_data_dir().join("USER.md"),
        FileType::Agents => workspace_data_dir().join("AGENTS.md"),
        FileType::Tools => workspace_data_dir().join("TOOLS.md"),
        FileType::Heartbeat => workspace_data_dir().join("HEARTBEAT.md"),
        FileType::Memory => workspace_data_dir().join("MEMORY.md"),
    }
}

/// Types of files that can be retrieved
#[derive(Debug, Clone, Copy)]
pub enum FileType {
    /// Main configuration file
    Config,
    /// Memory database
    MemoryDb,
    /// Log file
    Log,
    /// PID file
    Pid,
    /// SOUL.md personality file
    Soul,
    /// IDENTITY.md personality file
    Identity,
    /// BOOTSTRAP.md personality file
    Bootstrap,
    /// USER.md user-specific memory file
    User,
    /// AGENTS.md operating instructions file
    Agents,
    /// TOOLS.md tool notes and conventions file
    Tools,
    /// HEARTBEAT.md periodic task checklist file
    Heartbeat,
    /// MEMORY.md curated long-term memory file
    Memory,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manta_dir_structure() {
        // Just verify the paths are constructed correctly
        let base = manta_dir();
        assert!(base.to_string_lossy().contains(".manta"));

        assert!(config_dir().to_string_lossy().contains(".manta"));
        assert!(memory_dir().to_string_lossy().contains("memory"));
        assert!(logs_dir().to_string_lossy().contains("logs"));
        assert!(skills_dir().to_string_lossy().contains("skills"));
    }

    #[test]
    fn test_path_for() {
        assert!(path_for(FileType::Config)
            .to_string_lossy()
            .contains("manta.toml"));
        assert!(path_for(FileType::MemoryDb)
            .to_string_lossy()
            .contains("data/manta.db"));
        assert!(path_for(FileType::Log)
            .to_string_lossy()
            .contains("daemon.log"));
        assert!(path_for(FileType::Pid)
            .to_string_lossy()
            .contains("daemon.pid"));
    }

    #[test]
    fn test_path_for_workspace_files() {
        assert!(path_for(FileType::Soul)
            .to_string_lossy()
            .contains("SOUL.md"));
        assert!(path_for(FileType::Identity)
            .to_string_lossy()
            .contains("IDENTITY.md"));
        assert!(path_for(FileType::Bootstrap)
            .to_string_lossy()
            .contains("BOOTSTRAP.md"));
        assert!(path_for(FileType::User)
            .to_string_lossy()
            .contains("USER.md"));
        assert!(path_for(FileType::Agents)
            .to_string_lossy()
            .contains("AGENTS.md"));
        assert!(path_for(FileType::Tools)
            .to_string_lossy()
            .contains("TOOLS.md"));
        assert!(path_for(FileType::Heartbeat)
            .to_string_lossy()
            .contains("HEARTBEAT.md"));
        assert!(path_for(FileType::Memory)
            .to_string_lossy()
            .contains("MEMORY.md"));
    }

    #[test]
    fn test_default_memory_db() {
        let db = default_memory_db();
        assert!(db.to_string_lossy().contains("data/manta.db"));
    }

    #[test]
    fn test_pid_file() {
        let pid = pid_file();
        assert!(pid.to_string_lossy().contains("daemon.pid"));
    }

    #[test]
    fn test_default_log_file() {
        let log = default_log_file();
        assert!(log.to_string_lossy().contains("daemon.log"));
    }

    #[test]
    fn test_workspace_state_file() {
        let state = workspace_state_file();
        assert!(state.to_string_lossy().contains("workspace-state.json"));
    }

    #[test]
    fn test_transcripts_dir() {
        assert!(transcripts_dir().to_string_lossy().contains("transcripts"));
    }

    #[test]
    fn test_artifacts_dir() {
        assert!(artifacts_dir().to_string_lossy().contains("artifacts"));
    }

    #[test]
    fn test_budget_dir() {
        assert!(budget_dir().to_string_lossy().contains("budget"));
    }

    #[test]
    fn test_session_files_dir() {
        assert!(session_files_dir()
            .to_string_lossy()
            .contains("session_files"));
    }

    #[test]
    fn test_groups_dir() {
        assert!(groups_dir().to_string_lossy().contains("groups"));
    }

    #[test]
    fn test_teams_dir() {
        assert!(teams_dir().to_string_lossy().contains("teams"));
    }

    #[test]
    fn test_extensions_dir() {
        assert!(extensions_dir().to_string_lossy().contains("extensions"));
    }

    #[test]
    fn test_is_initialized() {
        // Just verify it doesn't panic
        let _ = is_initialized();
    }
}
