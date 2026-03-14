//! Centralized directory management for Manta
//!
//! All Manta data is stored in ~/.manta/ with the following structure:
//! ~/.manta/
//! ├── config/          # Configuration files (manta.toml)
//! ├── memory/          # SQLite databases (memory.db, chat history)
//! ├── memory-files/    # OpenClaw-style personality files (SOUL.md, IDENTITY.md, BOOTSTRAP.md)
//! ├── logs/            # Log files (daemon.log)
//! ├── skills/          # User-installed skills
//! ├── agents/          # Agent configurations
//! ├── cron/            # Cron job data
//! └── workspace/       # Workspace-level data

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

/// Get the config directory (~/.manta/config)
pub fn config_dir() -> PathBuf {
    manta_dir().join("config")
}

/// Get the memory/database directory (~/.manta/memory)
pub fn memory_dir() -> PathBuf {
    manta_dir().join("memory")
}

/// Get the memory files directory for OpenClaw-style files (~/.manta/memory-files)
pub fn memory_files_dir() -> PathBuf {
    manta_dir().join("memory-files")
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

/// Get the PID file path (~/.manta/daemon.pid)
pub fn pid_file() -> PathBuf {
    manta_dir().join("daemon.pid")
}

/// Get the default config file path (~/.manta/config/manta.toml)
pub fn default_config_file() -> PathBuf {
    config_dir().join("manta.toml")
}

/// Get the default memory DB path (~/.manta/memory/memory.db)
pub fn default_memory_db() -> PathBuf {
    memory_dir().join("memory.db")
}

/// Get the default log file path (~/.manta/logs/daemon.log)
pub fn default_log_file() -> PathBuf {
    logs_dir().join("daemon.log")
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
        &config_dir(),
        &memory_dir(),
        &memory_files_dir(),
        &logs_dir(),
        &skills_dir(),
        &agents_dir(),
        &cron_dir(),
        &workspace_data_dir(),
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
        &config_dir(),
        &memory_dir(),
        &memory_files_dir(),
        &logs_dir(),
        &skills_dir(),
        &agents_dir(),
        &cron_dir(),
        &workspace_data_dir(),
    ];

    for dir in &dirs {
        if !dir.exists() {
            debug!("Creating directory: {:?}", dir);
            std::fs::create_dir_all(dir).map_err(|e| {
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
        FileType::Soul => memory_files_dir().join("SOUL.md"),
        FileType::Identity => memory_files_dir().join("IDENTITY.md"),
        FileType::Bootstrap => memory_files_dir().join("BOOTSTRAP.md"),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manta_dir_structure() {
        // Just verify the paths are constructed correctly
        let base = manta_dir();
        assert!(base.to_string_lossy().contains(".manta"));

        assert!(config_dir().to_string_lossy().contains("config"));
        assert!(memory_dir().to_string_lossy().contains("memory"));
        assert!(logs_dir().to_string_lossy().contains("logs"));
        assert!(skills_dir().to_string_lossy().contains("skills"));
    }

    #[test]
    fn test_path_for() {
        assert!(path_for(FileType::Config).to_string_lossy().contains("manta.toml"));
        assert!(path_for(FileType::MemoryDb).to_string_lossy().contains("memory.db"));
        assert!(path_for(FileType::Log).to_string_lossy().contains("daemon.log"));
        assert!(path_for(FileType::Pid).to_string_lossy().contains("daemon.pid"));
    }
}
