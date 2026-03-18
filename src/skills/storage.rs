//! Multi-level skill storage
//!
//! Manages skills at multiple levels:
//! - Bundled: Built-in skills shipped with Manta
//! - User: Skills in ~/.manta/skills/
//! - Project: Skills in ./.manta/skills/ (current project)
//! - Workspace: Skills in workspace root

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Skill storage levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageLevel {
    /// Built-in skills (highest priority for availability)
    Bundled,
    /// User-level skills in ~/.manta/skills/
    User,
    /// Workspace-level skills
    Workspace,
    /// Project-level skills in ./.manta/skills/ (highest override priority)
    Project,
}

impl Default for StorageLevel {
    fn default() -> Self {
        StorageLevel::User
    }
}

impl std::fmt::Display for StorageLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl StorageLevel {
    /// Get the priority (lower = higher priority for loading)
    pub fn priority(&self) -> u8 {
        match self {
            StorageLevel::Bundled => 0,
            StorageLevel::User => 1,
            StorageLevel::Workspace => 2,
            StorageLevel::Project => 3,
        }
    }

    /// Get display name
    pub fn name(&self) -> &'static str {
        match self {
            StorageLevel::Bundled => "bundled",
            StorageLevel::User => "user",
            StorageLevel::Workspace => "workspace",
            StorageLevel::Project => "project",
        }
    }
}

/// Skill location info
#[derive(Debug, Clone)]
pub struct SkillLocation {
    /// Storage level
    pub level: StorageLevel,
    /// Path to the skill directory
    pub path: PathBuf,
    /// Skill name (directory name)
    pub name: String,
    /// Path to SKILL.md file
    pub skill_file: PathBuf,
}

/// Multi-level skill storage
pub struct SkillStorage {
    /// Bundled skills directory
    bundled_dir: Option<PathBuf>,
    /// User skills directory
    user_dir: PathBuf,
    /// Project skills directory (./.manta/skills/)
    project_dir: Option<PathBuf>,
    /// Workspace skills directory
    workspace_dir: Option<PathBuf>,
}

impl SkillStorage {
    /// Create a new skill storage instance
    pub fn new() -> crate::Result<Self> {
        let user_dir = Self::user_skills_dir()?;

        Ok(Self {
            bundled_dir: Self::bundled_skills_dir(),
            user_dir,
            project_dir: Self::project_skills_dir(),
            workspace_dir: Self::workspace_skills_dir(),
        })
    }

    /// Get the bundled skills directory
    fn bundled_skills_dir() -> Option<PathBuf> {
        // Try to find bundled skills relative to executable
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join("skills")))
            .or_else(|| {
                // Fallback: check for skills in source tree during development
                Some(PathBuf::from("./skills"))
            })
            .filter(|p| p.exists())
    }

    /// Get the user skills directory (~/.manta/skills/)
    fn user_skills_dir() -> crate::Result<PathBuf> {
        // Use centralized ~/.manta/skills directory
        Ok(crate::dirs::skills_dir())
    }

    /// Get the project skills directory (./.manta/skills/)
    fn project_skills_dir() -> Option<PathBuf> {
        std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(".manta").join("skills"))
            .filter(|p| p.exists())
    }

    /// Get the workspace skills directory
    fn workspace_skills_dir() -> Option<PathBuf> {
        // Look for workspace root marker
        let cwd = std::env::current_dir().ok()?;
        let mut current = cwd.as_path();

        loop {
            // Check for workspace markers
            let markers = [".manta-workspace", ".git", "manta.workspace.toml"];
            for marker in &markers {
                if current.join(marker).exists() {
                    let workspace_skills = current.join(".manta").join("skills");
                    if workspace_skills.exists()
                        && workspace_skills != cwd.join(".manta").join("skills")
                    {
                        return Some(workspace_skills);
                    }
                }
            }

            // Go up one level
            match current.parent() {
                Some(parent) => current = parent,
                None => break,
            }
        }

        None
    }

    /// Ensure user skills directory exists
    pub async fn ensure_user_dir(&self) -> crate::Result<()> {
        tokio::fs::create_dir_all(&self.user_dir)
            .await
            .map_err(|e| crate::error::MantaError::Io(e))?;
        Ok(())
    }

    /// Ensure project skills directory exists
    pub async fn ensure_project_dir(&self) -> crate::Result<PathBuf> {
        let dir = std::env::current_dir()
            .map_err(|e| crate::error::MantaError::Io(e))?
            .join(".manta")
            .join("skills");

        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| crate::error::MantaError::Io(e))?;

        Ok(dir)
    }

    /// Get the path for a skill at a specific level
    pub fn skill_path(&self, name: &str, level: StorageLevel) -> Option<PathBuf> {
        let base = match level {
            StorageLevel::Bundled => self.bundled_dir.as_ref()?,
            StorageLevel::User => &self.user_dir,
            StorageLevel::Project => self.project_dir.as_ref()?,
            StorageLevel::Workspace => self.workspace_dir.as_ref()?,
        };

        Some(base.join(name))
    }

    /// Get the SKILL.md path for a skill
    pub fn skill_file_path(&self, name: &str, level: StorageLevel) -> Option<PathBuf> {
        self.skill_path(name, level).map(|p| p.join("SKILL.md"))
    }

    /// Discover all skills across all levels
    pub async fn discover_all(&self) -> Vec<SkillLocation> {
        let mut skills = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Discover in order of priority (bundled first, then override)
        for level in [
            StorageLevel::Bundled,
            StorageLevel::User,
            StorageLevel::Workspace,
            StorageLevel::Project,
        ] {
            let discovered = self.discover_at_level(level).await;

            for skill in discovered {
                // Higher priority levels override lower ones
                if seen.insert(skill.name.clone()) {
                    debug!("Found skill '{}' at {:?} level", skill.name, level);
                    skills.push(skill);
                } else {
                    debug!("Skill '{}' at {:?} level overrides lower priority", skill.name, level);
                }
            }
        }

        skills
    }

    /// Discover skills at a specific level
    pub async fn discover_at_level(&self, level: StorageLevel) -> Vec<SkillLocation> {
        let base_dir = match level {
            StorageLevel::Bundled => match &self.bundled_dir {
                Some(d) => d.clone(),
                None => return Vec::new(),
            },
            StorageLevel::User => self.user_dir.clone(),
            StorageLevel::Project => match &self.project_dir {
                Some(d) => d.clone(),
                None => return Vec::new(),
            },
            StorageLevel::Workspace => match &self.workspace_dir {
                Some(d) => d.clone(),
                None => return Vec::new(),
            },
        };

        if !base_dir.exists() {
            return Vec::new();
        }

        let mut skills = Vec::new();

        match tokio::fs::read_dir(&base_dir).await {
            Ok(mut entries) => {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();

                    if path.is_dir() {
                        let skill_file = path.join("SKILL.md");

                        if skill_file.exists() {
                            let name = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("unknown")
                                .to_string();

                            skills.push(SkillLocation { level, path, name, skill_file });
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to read skills directory {:?}: {}", base_dir, e);
            }
        }

        skills
    }

    /// Install a skill from a directory to user level
    pub async fn install_to_user(&self, source_dir: &Path, name: &str) -> crate::Result<PathBuf> {
        let dest = self.user_dir.join(name);

        info!("Installing skill '{}' from {:?} to {:?}", name, source_dir, dest);

        // Remove existing if present
        if dest.exists() {
            tokio::fs::remove_dir_all(&dest)
                .await
                .map_err(|e| crate::error::MantaError::Io(e))?;
        }

        // Copy directory
        copy_dir_recursive(source_dir, &dest).await?;

        info!("Successfully installed skill '{}' to {:?}", name, dest);
        Ok(dest)
    }

    /// Remove a skill from user level
    pub async fn uninstall_from_user(&self, name: &str) -> crate::Result<()> {
        let path = self.user_dir.join(name);

        if !path.exists() {
            return Err(crate::error::MantaError::NotFound {
                resource: format!("Skill '{}' not found at {:?}", name, path),
            });
        }

        tokio::fs::remove_dir_all(&path)
            .await
            .map_err(|e| crate::error::MantaError::Io(e))?;

        info!("Uninstalled skill '{}' from {:?}", name, path);
        Ok(())
    }

    /// Get the storage level for a skill
    pub async fn get_skill_level(&self, name: &str) -> Option<StorageLevel> {
        // Check in priority order
        for level in [
            StorageLevel::Project,
            StorageLevel::Workspace,
            StorageLevel::User,
            StorageLevel::Bundled,
        ] {
            if let Some(path) = self.skill_file_path(name, level) {
                if path.exists() {
                    return Some(level);
                }
            }
        }
        None
    }

    /// Get the user skills directory path
    pub fn user_dir(&self) -> &Path {
        &self.user_dir
    }

    /// Get all storage directory paths
    pub fn get_all_paths(&self) -> Vec<(StorageLevel, PathBuf)> {
        let mut paths = Vec::new();

        if let Some(ref dir) = self.bundled_dir {
            paths.push((StorageLevel::Bundled, dir.clone()));
        }
        paths.push((StorageLevel::User, self.user_dir.clone()));
        if let Some(ref dir) = self.workspace_dir {
            paths.push((StorageLevel::Workspace, dir.clone()));
        }
        if let Some(ref dir) = self.project_dir {
            paths.push((StorageLevel::Project, dir.clone()));
        }

        paths
    }

    /// List all skills with their levels
    pub async fn list_with_levels(&self) -> HashMap<String, StorageLevel> {
        let mut map = HashMap::new();

        for level in [
            StorageLevel::Bundled,
            StorageLevel::User,
            StorageLevel::Workspace,
            StorageLevel::Project,
        ] {
            let skills = self.discover_at_level(level).await;
            for skill in skills {
                // Higher priority levels override
                map.insert(skill.name, level);
            }
        }

        map
    }

    /// Get all storage directories
    pub fn all_dirs(&self) -> Vec<(StorageLevel, PathBuf)> {
        let mut dirs = Vec::new();

        if let Some(ref d) = self.bundled_dir {
            dirs.push((StorageLevel::Bundled, d.clone()));
        }
        dirs.push((StorageLevel::User, self.user_dir.clone()));
        if let Some(ref d) = self.workspace_dir {
            dirs.push((StorageLevel::Workspace, d.clone()));
        }
        if let Some(ref d) = self.project_dir {
            dirs.push((StorageLevel::Project, d.clone()));
        }

        dirs
    }

    /// Refresh project and workspace directories (useful after cwd change)
    pub fn refresh(&mut self) {
        self.project_dir = Self::project_skills_dir();
        self.workspace_dir = Self::workspace_skills_dir();
    }
}

impl Default for SkillStorage {
    fn default() -> Self {
        // Safe unwrap - user_skills_dir only fails if no home directory
        Self::new().expect("Failed to create skill storage")
    }
}

/// Copy directory recursively
async fn copy_dir_recursive(src: &Path, dst: &Path) -> crate::Result<()> {
    tokio::fs::create_dir_all(dst)
        .await
        .map_err(|e| crate::error::MantaError::Io(e))?;

    let mut entries = tokio::fs::read_dir(src)
        .await
        .map_err(|e| crate::error::MantaError::Io(e))?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path)
                .await
                .map_err(|e| crate::error::MantaError::Io(e))?;
        }
    }

    Ok(())
}

/// Find the project root (directory containing .manta/)
#[allow(dead_code)]
pub fn find_project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut current = cwd.as_path();

    loop {
        if current.join(".manta").is_dir() {
            return Some(current.to_path_buf());
        }

        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    None
}

/// Find the workspace root
#[allow(dead_code)]
pub fn find_workspace_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut current = cwd.as_path();

    loop {
        // Check for workspace markers
        let markers = [".manta-workspace", ".git", "manta.workspace.toml"];
        for marker in &markers {
            if current.join(marker).exists() {
                return Some(current.to_path_buf());
            }
        }

        match current.parent() {
            Some(parent) => current = parent,
            None => break,
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_level_priority() {
        assert_eq!(StorageLevel::Bundled.priority(), 0);
        assert_eq!(StorageLevel::User.priority(), 1);
        assert_eq!(StorageLevel::Workspace.priority(), 2);
        assert_eq!(StorageLevel::Project.priority(), 3);
    }

    #[test]
    fn test_storage_level_name() {
        assert_eq!(StorageLevel::Bundled.name(), "bundled");
        assert_eq!(StorageLevel::User.name(), "user");
        assert_eq!(StorageLevel::Workspace.name(), "workspace");
        assert_eq!(StorageLevel::Project.name(), "project");
    }

    #[test]
    fn test_user_skills_dir() {
        let dir = SkillStorage::user_skills_dir();
        assert!(dir.is_ok());
        let dir = dir.unwrap();
        assert!(dir.to_string_lossy().contains("manta"));
        assert!(dir.to_string_lossy().contains("skills"));
    }
}
