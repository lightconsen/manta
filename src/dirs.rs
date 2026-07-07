//! Centralized directory management for Syscity
//!
//! All Syscity data is stored in ~/.syscity/ with the following structure:
//! ~/.syscity/
//! ├── config.toml # Configuration file
//! ├── data/ # SQLite database (syscity.db) - unified storage
//! ├── logs/ # Log files (daemon.log)
//! ├── skills/ # User-installed skills
//! ├── agents/ # Agent configurations
//! │ └── {agent-id}/
//! │ ├── personality.toml # Agent configuration
//! │ ├── workspace/ # Agent-specific workspace (AI file ops)
//! │ └── data/ # Agent runtime data (sessions, state)
//! ├── cron/ # Cron job data
//! ├── todos/ # Task persistence
//! ├── workspace/ # Default workspace for AI file operations
//! │ # (also holds SOUL.md, IDENTITY.md, BOOTSTRAP.md, USER.md)
//! └── memory/ # Legacy directory (deprecated, kept for backward compatibility)

use std::path::{Path, PathBuf};

use tracing::{debug, info};

/// Base directory name
const SYSCITY_DIR: &str = ".syscity";

/// Get the home directory
fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Get the base Syscity directory (~/.syscity)
pub fn syscity_dir() -> PathBuf {
    home_dir()
        .map(|h| h.join(SYSCITY_DIR))
        .unwrap_or_else(|| PathBuf::from(SYSCITY_DIR))
}

/// Get the config directory (~/.syscity)
pub fn config_dir() -> PathBuf {
    syscity_dir()
}

/// Get the memory/database directory (~/.syscity/memory)
pub fn memory_dir() -> PathBuf {
    syscity_dir().join("memory")
}

/// Get the workspace data directory for files (~/.syscity/workspace)
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

/// Get the logs directory (~/.syscity/logs)
pub fn logs_dir() -> PathBuf {
    syscity_dir().join("logs")
}

/// Get the skills directory (~/.syscity/skills)
pub fn skills_dir() -> PathBuf {
    syscity_dir().join("skills")
}

/// Get the agents directory (~/.syscity/agents)
pub fn agents_dir() -> PathBuf {
    syscity_dir().join("agents")
}

/// Get a specific agent's base directory (~/.syscity/agents/{id})
pub fn agent_dir(agent_id: &str) -> PathBuf {
    agents_dir().join(agent_id)
}

/// Get a specific agent's workspace directory
/// (~/.syscity/agents/{id}/workspace)
pub fn agent_workspace_dir(agent_id: &str) -> PathBuf {
    agent_dir(agent_id).join("workspace")
}

/// Get a specific agent's data directory (~/.syscity/agents/{id}/data)
pub fn agent_data_dir(agent_id: &str) -> PathBuf {
    agent_dir(agent_id).join("data")
}

/// Resolve a path, expanding `~` to the user's home directory.
///
/// If the path starts with `~` or `~/`, it is expanded using the home
/// directory. Otherwise, the path is returned unchanged.
pub fn resolve_tilde(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if let Some(path_str) = path.to_str() {
        if let Some(rest) = path_str.strip_prefix("~/") {
            if let Some(home) = home_dir() {
                return home.join(rest);
            }
        } else if path_str == "~" {
            if let Some(home) = home_dir() {
                return home;
            }
        }
    }
    path.to_path_buf()
}

/// Get the cron directory (~/.syscity/cron)
pub fn cron_dir() -> PathBuf {
    syscity_dir().join("cron")
}

/// Get the workspace data directory (~/.syscity/workspace)
pub fn workspace_data_dir() -> PathBuf {
    syscity_dir().join("workspace")
}

/// Get the data directory (~/.syscity/data)
pub fn data_dir() -> PathBuf {
    syscity_dir().join("data")
}

/// Get the todos directory (~/.syscity/todos)
pub fn todos_dir() -> PathBuf {
    syscity_dir().join("todos")
}
pub fn teams_dir() -> PathBuf {
    syscity_dir().join("teams")
}

/// Get the plugins data directory (~/.syscity/plugins/data)
pub fn plugins_data_dir() -> PathBuf {
    syscity_dir().join("plugins").join("data")
}

/// Get the extensions directory (~/.syscity/extensions)
pub fn extensions_dir() -> PathBuf {
    syscity_dir().join("extensions")
}

/// Get the transcripts directory (~/.syscity/transcripts)
pub fn transcripts_dir() -> PathBuf {
    syscity_dir().join("transcripts")
}

/// Get the artifacts directory (~/.syscity/artifacts)
pub fn artifacts_dir() -> PathBuf {
    syscity_dir().join("artifacts")
}

/// Get the disk budget tracking directory (~/.syscity/budget)
pub fn budget_dir() -> PathBuf {
    syscity_dir().join("budget")
}

/// Get the session files directory (~/.syscity/session_files)
pub fn session_files_dir() -> PathBuf {
    syscity_dir().join("session_files")
}

/// Get the group sessions directory (~/.syscity/groups)
pub fn groups_dir() -> PathBuf {
    syscity_dir().join("groups")
}

/// Get the PID file path (~/.syscity/daemon.pid)
pub fn pid_file() -> PathBuf {
    syscity_dir().join("daemon.pid")
}

/// Get the default config file path (~/.syscity/config.toml)
pub fn default_config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// Get the default memory DB path (~/.syscity/data/syscity.db)
///
/// Note: Previously returned ~/.syscity/memory/memory.db, now consolidated
/// to use the main gateway database for unified storage.
pub fn default_memory_db() -> PathBuf {
    data_dir().join("syscity.db")
}

/// Get the default log file path (~/.syscity/logs/daemon.log)
pub fn default_log_file() -> PathBuf {
    logs_dir().join("daemon.log")
}

/// Get the workspace state file path
/// (~/.syscity/workspace/.syscity/workspace-state.json)
pub fn workspace_state_file() -> PathBuf {
    workspace_data_dir()
        .join(".syscity")
        .join("workspace-state.json")
}

/// Initialize all Syscity directories
///
/// Creates the ~/.syscity directory structure if it doesn't exist.
/// Returns the base directory path.
pub async fn init() -> crate::Result<PathBuf> {
    let base = syscity_dir();

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
        &plugins_data_dir(),
    ];

    for dir in &dirs {
        if !dir.exists() {
            debug!("Creating directory: {:?}", dir);
            tokio::fs::create_dir_all(dir).await.map_err(|e| {
                crate::error::SyscityError::Storage {
                    context: format!("Failed to create directory: {:?}", dir),
                    details: e.to_string(),
                }
            })?;
        }
    }

    // Seed default agent personality templates
    seed_default_agent_personality(&base).await?;

    info!("Syscity directories initialized at: {:?}", base);
    Ok(base)
}

/// Seed the default agent (`agents/default/`) with standard
/// personality files if they don't already exist.
async fn seed_default_agent_personality(base: &Path) -> crate::Result<()> {
    let default_agent_dir = base.join("agents").join("default");
    let params = crate::agent::AgentTemplateParams::default();
    crate::agent::seed_agent_personality(&default_agent_dir, &params).await
}

/// Initialize directories synchronously (for non-async contexts)
pub fn init_sync() -> crate::Result<PathBuf> {
    let base = syscity_dir();

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
        &plugins_data_dir(),
    ];

    for dir in &dirs {
        if !dir.exists() {
            debug!("Creating directory: {:?}", dir);
            std::fs::create_dir_all(dir).map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to create directory: {:?}", dir),
                details: e.to_string(),
            })?;
        }
    }

    // Seed default agent personality templates (sync)
    seed_default_agent_personality_sync(&base)?;

    info!("Syscity directories initialized at: {:?}", base);
    Ok(base)
}

/// Synchronous version of `seed_default_agent_personality`.
fn seed_default_agent_personality_sync(base: &Path) -> crate::Result<()> {
    let default_agent_dir = base.join("agents").join("default");
    let params = crate::agent::AgentTemplateParams::default();
    crate::agent::seed_agent_personality_sync(&default_agent_dir, &params)
}

/// Check if Syscity directories are initialized
pub fn is_initialized() -> bool {
    syscity_dir().exists()
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
    fn test_syscity_dir_structure() {
        // Just verify the paths are constructed correctly
        let base = syscity_dir();
        assert!(base.to_string_lossy().contains(".syscity"));

        assert!(config_dir().to_string_lossy().contains(".syscity"));
        assert!(memory_dir().to_string_lossy().contains("memory"));
        assert!(logs_dir().to_string_lossy().contains("logs"));
        assert!(skills_dir().to_string_lossy().contains("skills"));
    }

    #[test]
    fn test_path_for() {
        assert!(path_for(FileType::Config)
            .to_string_lossy()
            .contains("config.toml"));
        assert!(path_for(FileType::MemoryDb)
            .to_string_lossy()
            .contains("data/syscity.db"));
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
        assert!(db.to_string_lossy().contains("data/syscity.db"));
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
    fn test_plugins_data_dir() {
        let dir = plugins_data_dir();
        assert!(dir.to_string_lossy().contains("plugins"));
        assert!(dir.to_string_lossy().contains("data"));
    }

    #[test]
    fn test_is_initialized() {
        // Just verify it doesn't panic
        let _ = is_initialized();
    }

    #[test]
    fn test_resolve_tilde_home() {
        let home = home_dir().unwrap();
        assert_eq!(resolve_tilde("~"), home);
    }

    #[test]
    fn test_resolve_tilde_home_subdir() {
        let home = home_dir().unwrap();
        assert_eq!(resolve_tilde("~/projects"), home.join("projects"));
    }

    #[test]
    fn test_resolve_tilde_no_tilde() {
        let path = "/usr/local/bin";
        assert_eq!(resolve_tilde(path), PathBuf::from(path));
    }

    #[test]
    fn test_agent_dir() {
        let base = agents_dir();
        assert_eq!(agent_dir("my-agent"), base.join("my-agent"));
    }

    #[test]
    fn test_agent_workspace_dir() {
        assert_eq!(
            agent_workspace_dir("my-agent"),
            agents_dir().join("my-agent").join("workspace")
        );
    }

    #[test]
    fn test_agent_data_dir() {
        assert_eq!(agent_data_dir("my-agent"), agents_dir().join("my-agent").join("data"));
    }
}
