//! Agent Team Management Module
//!
//! This module provides team management for organizing multiple agents
//! into coordinated groups with defined hierarchies and communication patterns.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, error, info, warn};

pub mod mesh;

pub use mesh::{get_team_mesh_manager, TeamMeshManager, TeamMeshSession, TeamMessageResult};

/// Team configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    /// Team name (unique identifier)
    pub name: String,
    /// Team description
    pub description: Option<String>,
    /// Team type
    #[serde(default)]
    pub team_type: TeamType,
    /// Team members
    #[serde(default)]
    pub members: HashMap<String, TeamMember>,
    /// Hierarchy structure (manager -> [workers])
    #[serde(default)]
    pub hierarchy: HashMap<String, Vec<String>>,
    /// Communication pattern
    #[serde(default)]
    pub communication: CommunicationPattern,
    /// Shared memory/canvas name
    pub shared_memory: Option<String>,
    /// Whether team is active
    #[serde(default)]
    pub active: bool,
    /// Creation timestamp
    pub created_at: String,
    /// Last updated timestamp
    pub updated_at: String,
}

/// Team type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum TeamType {
    /// Flat structure - all agents are peers
    #[default]
    Flat,
    /// Hierarchical - managers and workers
    Hierarchical,
    /// Network - agents connect as needed
    Network,
}

/// Communication pattern
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum CommunicationPattern {
    /// Broadcast - all messages go to all agents
    #[default]
    Broadcast,
    /// Chain - messages flow through a chain
    Chain,
    /// Star - central coordinator distributes messages
    Star,
    /// Mesh - agents communicate directly
    Mesh,
}

/// Team member
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    /// Agent name
    pub name: String,
    /// Role in the team
    pub role: String,
    /// Hierarchy level (0 = top, higher = lower)
    pub level: u8,
    /// Can delegate tasks
    #[serde(default)]
    pub can_delegate: bool,
    /// Agent capabilities/tools
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Joined timestamp
    pub joined_at: String,
}

impl Team {
    /// Create a new team
    pub fn new(name: impl Into<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            name: name.into(),
            description: None,
            team_type: TeamType::default(),
            members: HashMap::new(),
            hierarchy: HashMap::new(),
            communication: CommunicationPattern::default(),
            shared_memory: None,
            active: false,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Add a member to the team
    pub fn add_member(&mut self, name: impl Into<String>, role: impl Into<String>) {
        let name = name.into();
        let member = TeamMember {
            name: name.clone(),
            role: role.into(),
            level: 1,
            can_delegate: false,
            capabilities: vec![],
            joined_at: chrono::Utc::now().to_rfc3339(),
        };
        self.members.insert(name, member);
        self.update_timestamp();
    }

    /// Remove a member from the team
    pub fn remove_member(&mut self, name: &str) -> Option<TeamMember> {
        let removed = self.members.remove(name);
        if removed.is_some() {
            // Also remove from hierarchy
            self.hierarchy.remove(name);
            for managed in self.hierarchy.values_mut() {
                managed.retain(|m| m != name);
            }
            self.update_timestamp();
        }
        removed
    }

    /// Set member role
    pub fn set_role(&mut self, name: &str, role: impl Into<String>) -> crate::Result<()> {
        if let Some(member) = self.members.get_mut(name) {
            member.role = role.into();
            self.update_timestamp();
            Ok(())
        } else {
            Err(crate::error::MantaError::Validation(format!(
                "Member '{}' not found in team '{}'",
                name, self.name
            )))
        }
    }

    /// Set member level
    pub fn set_level(&mut self, name: &str, level: u8) -> crate::Result<()> {
        if let Some(member) = self.members.get_mut(name) {
            member.level = level;
            self.update_timestamp();
            Ok(())
        } else {
            Err(crate::error::MantaError::Validation(format!(
                "Member '{}' not found in team '{}'",
                name, self.name
            )))
        }
    }

    /// Set delegation capability
    pub fn set_can_delegate(&mut self, name: &str, can_delegate: bool) -> crate::Result<()> {
        if let Some(member) = self.members.get_mut(name) {
            member.can_delegate = can_delegate;
            self.update_timestamp();
            Ok(())
        } else {
            Err(crate::error::MantaError::Validation(format!(
                "Member '{}' not found in team '{}'",
                name, self.name
            )))
        }
    }

    /// Set hierarchy structure
    /// Format: "manager:worker1,worker2;manager2:worker3,worker4"
    pub fn set_hierarchy(&mut self, structure: &str) -> crate::Result<()> {
        self.hierarchy.clear();

        // Support multiple hierarchy definitions separated by semicolon
        for part in structure.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            // Find the colon separator (only split on first colon)
            let colon_pos = part.find(':').ok_or_else(|| {
                crate::error::MantaError::Validation(format!(
                    "Invalid hierarchy format: '{}' (expected 'manager:worker1,worker2')",
                    part
                ))
            })?;

            let manager = part[..colon_pos].trim().to_string();
            let workers_str = &part[colon_pos + 1..];

            let workers: Vec<String> = workers_str
                .split(',')
                .map(|w| w.trim().to_string())
                .filter(|w| !w.is_empty())
                .collect();

            // Validate that all agents exist
            if !self.members.contains_key(&manager) {
                return Err(crate::error::MantaError::Validation(format!(
                    "Manager '{}' is not a team member",
                    manager
                )));
            }
            for worker in &workers {
                if !self.members.contains_key(worker) {
                    return Err(crate::error::MantaError::Validation(format!(
                        "Worker '{}' is not a team member",
                        worker
                    )));
                }
            }

            self.hierarchy.insert(manager, workers);
        }

        self.update_timestamp();
        Ok(())
    }

    /// Set communication pattern
    pub fn set_communication(
        &mut self,
        pattern: CommunicationPattern,
        shared_memory: Option<String>,
    ) {
        self.communication = pattern;
        self.shared_memory = shared_memory;
        self.update_timestamp();
    }

    /// Get team leads (agents at level 0 or those who manage others)
    pub fn get_leads(&self) -> Vec<&TeamMember> {
        self.members
            .values()
            .filter(|m| m.level == 0 || self.hierarchy.contains_key(&m.name))
            .collect()
    }

    /// Get agents who can delegate
    pub fn get_delegators(&self) -> Vec<&TeamMember> {
        self.members.values().filter(|m| m.can_delegate).collect()
    }

    /// Update timestamp
    fn update_timestamp(&mut self) {
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Get the team configuration file path
    pub fn config_path(&self) -> PathBuf {
        crate::dirs::teams_dir().join(&self.name).join("team.yaml")
    }

    /// Save team to disk
    pub async fn save(&self) -> crate::Result<()> {
        let team_dir = crate::dirs::teams_dir().join(&self.name);
        tokio::fs::create_dir_all(&team_dir).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: format!("Failed to create team directory: {:?}", team_dir),
                details: e.to_string(),
            }
        })?;

        let config_path = team_dir.join("team.yaml");
        let yaml = serde_yaml::to_string(self).map_err(|e| {
            crate::error::MantaError::Config(crate::error::ConfigError::Parse(format!(
                "YAML error: {}",
                e
            )))
        })?;

        tokio::fs::write(&config_path, yaml).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: format!("Failed to write team config: {:?}", config_path),
                details: e.to_string(),
            }
        })?;

        info!("Saved team '{}' to {:?}", self.name, config_path);
        Ok(())
    }

    /// Load team from disk
    pub async fn load(name: &str) -> crate::Result<Self> {
        let config_path = crate::dirs::teams_dir().join(name).join("team.yaml");

        let yaml = tokio::fs::read_to_string(&config_path).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: format!("Failed to read team config: {:?}", config_path),
                details: e.to_string(),
            }
        })?;

        let team: Team = serde_yaml::from_str(&yaml).map_err(|e| {
            crate::error::MantaError::Config(crate::error::ConfigError::Parse(format!(
                "YAML error: {}",
                e
            )))
        })?;

        Ok(team)
    }

    /// Delete team from disk
    pub async fn delete(&self) -> crate::Result<()> {
        let team_dir = crate::dirs::teams_dir().join(&self.name);

        if team_dir.exists() {
            tokio::fs::remove_dir_all(&team_dir).await.map_err(|e| {
                crate::error::MantaError::Storage {
                    context: format!("Failed to delete team directory: {:?}", team_dir),
                    details: e.to_string(),
                }
            })?;
        }

        info!("Deleted team '{}' from {:?}", self.name, team_dir);
        Ok(())
    }

    /// List all teams
    pub async fn list_all() -> crate::Result<Vec<String>> {
        let teams_dir = crate::dirs::teams_dir();

        if !teams_dir.exists() {
            return Ok(vec![]);
        }

        let mut teams = vec![];
        let mut entries = tokio::fs::read_dir(&teams_dir).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: format!("Failed to read teams directory: {:?}", teams_dir),
                details: e.to_string(),
            }
        })?;

        while let Some(entry) =
            entries
                .next_entry()
                .await
                .map_err(|e| crate::error::MantaError::Storage {
                    context: format!("Failed to read directory entry"),
                    details: e.to_string(),
                })?
        {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    // Check if team.yaml exists
                    if path.join("team.yaml").exists() {
                        teams.push(name.to_string());
                    }
                }
            }
        }

        Ok(teams)
    }

    /// Export team to string (YAML or JSON)
    pub fn export(&self, format: &str) -> crate::Result<String> {
        match format.to_lowercase().as_str() {
            "json" => serde_json::to_string_pretty(self)
                .map_err(|e| crate::error::MantaError::Serialization(e)),
            "yaml" | "yml" => serde_yaml::to_string(self).map_err(|e| {
                crate::error::MantaError::Config(crate::error::ConfigError::Parse(format!(
                    "YAML serialization error: {}",
                    e
                )))
            }),
            _ => Err(crate::error::MantaError::Validation(format!(
                "Unsupported export format: '{}' (use 'yaml' or 'json')",
                format
            ))),
        }
    }

    /// Import team from string
    pub fn import(data: &str, format: &str, rename: Option<String>) -> crate::Result<Self> {
        let mut team: Team = match format.to_lowercase().as_str() {
            "json" => serde_json::from_str(data)
                .map_err(|e| crate::error::MantaError::Serialization(e))?,
            "yaml" | "yml" => serde_yaml::from_str(data).map_err(|e| {
                crate::error::MantaError::Config(crate::error::ConfigError::Parse(format!(
                    "YAML deserialization error: {}",
                    e
                )))
            })?,
            _ => {
                return Err(crate::error::MantaError::Validation(format!(
                    "Unsupported import format: '{}' (use 'yaml' or 'json')",
                    format
                )))
            }
        };

        if let Some(new_name) = rename {
            team.name = new_name;
        }

        // Reset timestamps
        let now = chrono::Utc::now().to_rfc3339();
        team.created_at = now.clone();
        team.updated_at = now;
        team.active = false;

        Ok(team)
    }
}

impl std::fmt::Display for TeamType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TeamType::Flat => write!(f, "flat"),
            TeamType::Hierarchical => write!(f, "hierarchical"),
            TeamType::Network => write!(f, "network"),
        }
    }
}

impl std::fmt::Display for CommunicationPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommunicationPattern::Broadcast => write!(f, "broadcast"),
            CommunicationPattern::Chain => write!(f, "chain"),
            CommunicationPattern::Star => write!(f, "star"),
            CommunicationPattern::Mesh => write!(f, "mesh"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_team_creation() {
        let team = Team::new("test-team");
        assert_eq!(team.name, "test-team");
        assert!(team.members.is_empty());
        assert!(!team.active);
    }

    #[test]
    fn test_add_member() {
        let mut team = Team::new("test-team");
        team.add_member("agent1", "worker");
        assert!(team.members.contains_key("agent1"));
        assert_eq!(team.members["agent1"].role, "worker");
    }

    #[test]
    fn test_remove_member() {
        let mut team = Team::new("test-team");
        team.add_member("agent1", "worker");
        let removed = team.remove_member("agent1");
        assert!(removed.is_some());
        assert!(!team.members.contains_key("agent1"));
    }

    #[test]
    fn test_hierarchy() {
        let mut team = Team::new("test-team");
        team.add_member("lead", "lead");
        team.add_member("worker1", "worker");
        team.add_member("worker2", "worker");

        team.set_hierarchy("lead:worker1,worker2").unwrap();
        assert_eq!(team.hierarchy["lead"], vec!["worker1", "worker2"]);
    }

    #[test]
    fn test_export_import_yaml() {
        let mut team = Team::new("test-team");
        team.description = Some("Test description".to_string());
        team.add_member("agent1", "worker");

        let yaml = team.export("yaml").unwrap();
        let imported = Team::import(&yaml, "yaml", None).unwrap();

        assert_eq!(imported.name, "test-team");
        assert_eq!(imported.description, Some("Test description".to_string()));
    }
}
