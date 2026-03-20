//! Agent Personality Discovery and Loading
//!
//! Inspired by OpenClaw's personality system, this provides:
//! - Automatic discovery of agents from `agents/` directory
//! - Loading of personality files (SOUL.md, IDENTITY.md, BOOTSTRAP.md, USER.md)
//! - Personality-based AgentConfig generation
//! - Agent registry for on-demand spawning

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, error, info, warn};

use crate::agent::AgentConfig;
use crate::dirs;

/// Maximum size for personality files (4KB default)
const DEFAULT_MAX_FILE_SIZE: usize = 4096;

/// Agent personality loaded from markdown files
#[derive(Debug, Clone, Default)]
pub struct AgentPersonality {
    /// Agent ID (directory name)
    pub id: String,
    /// SOUL.md - Core personality, values, behavioral guidelines
    pub soul: String,
    /// IDENTITY.md - Agent identity, name, role definition
    pub identity: String,
    /// BOOTSTRAP.md - Initial startup behavior, first-run logic
    pub bootstrap: String,
    /// USER.md - User-specific memory, preferences
    pub user: String,
    /// AGENTS.md - Operating instructions for other agents
    pub agents: String,
    /// TOOLS.md - Tool notes and conventions
    pub tools: String,
    /// Path to the agent directory
    pub path: PathBuf,
    /// Whether this personality is valid (has at least SOUL.md or IDENTITY.md)
    pub is_valid: bool,
}

impl AgentPersonality {
    /// Load personality from an agent directory
    pub async fn load(agent_dir: &Path) -> crate::Result<Self> {
        let id = agent_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        info!("Loading agent personality: {}", id);

        let mut personality = Self {
            id: id.clone(),
            path: agent_dir.to_path_buf(),
            ..Default::default()
        };

        // Load each personality file
        personality.soul = personality.load_file("SOUL.md").await;
        personality.identity = personality.load_file("IDENTITY.md").await;
        personality.bootstrap = personality.load_file("BOOTSTRAP.md").await;
        personality.user = personality.load_file("USER.md").await;
        personality.agents = personality.load_file("AGENTS.md").await;
        personality.tools = personality.load_file("TOOLS.md").await;

        // Valid if has SOUL.md or IDENTITY.md
        personality.is_valid = !personality.soul.is_empty() || !personality.identity.is_empty();

        if personality.is_valid {
            info!("✅ Loaded personality for agent '{}'", id);
        } else {
            warn!("⚠️  Agent '{}' has no SOUL.md or IDENTITY.md", id);
        }

        Ok(personality)
    }

    /// Load a specific file from the agent directory
    async fn load_file(&self, filename: &str) -> String {
        let file_path = self.path.join(filename);

        if !file_path.exists() {
            return String::new();
        }

        match fs::read_to_string(&file_path).await {
            Ok(content) => {
                // Truncate if too large
                if content.len() > DEFAULT_MAX_FILE_SIZE {
                    debug!(
                        "Personality file {} for agent {} exceeds {} bytes, truncating",
                        filename, self.id, DEFAULT_MAX_FILE_SIZE
                    );
                    content.chars().take(DEFAULT_MAX_FILE_SIZE).collect()
                } else {
                    content
                }
            }
            Err(e) => {
                warn!("Failed to read {} for agent {}: {}", filename, self.id, e);
                String::new()
            }
        }
    }

    /// Convert personality to AgentConfig
    pub fn to_agent_config(&self) -> AgentConfig {
        let system_prompt = self.build_system_prompt();

        AgentConfig {
            system_prompt,
            max_context_tokens: 4096,
            max_concurrent_tools: 5,
            temperature: 0.7,
            max_tokens: 2048,
            skills_prompt: None,
            max_turns: None,
            compaction_model: None,
        }
    }

    /// Build full system prompt from personality files
    /// Priority: BOOTSTRAP > IDENTITY > SOUL (OpenClaw-style)
    fn build_system_prompt(&self) -> String {
        let mut sections = Vec::new();

        // BOOTSTRAP.md - Initial behavior (highest priority)
        if !self.bootstrap.is_empty() {
            sections.push(format!("## Bootstrap\n{}\n", self.bootstrap.trim()));
        }

        // IDENTITY.md - Who the agent is
        if !self.identity.is_empty() {
            sections.push(format!("## Identity\n{}\n", self.identity.trim()));
        }

        // SOUL.md - Core personality
        if !self.soul.is_empty() {
            sections.push(format!("## Soul\n{}\n", self.soul.trim()));
        }

        // AGENTS.md - Operating instructions
        if !self.agents.is_empty() {
            sections.push(format!("## Agents\n{}\n", self.agents.trim()));
        }

        // TOOLS.md - Tool conventions
        if !self.tools.is_empty() {
            sections.push(format!("## Tools\n{}\n", self.tools.trim()));
        }

        if sections.is_empty() {
            // Fallback to default
            AgentConfig::default().system_prompt
        } else {
            sections.join("\n")
        }
    }

    /// Get the agent's display name from identity
    pub fn display_name(&self) -> String {
        // Try to extract name from IDENTITY.md first line
        self.identity
            .lines()
            .next()
            .and_then(|line| {
                line.strip_prefix("#")
                    .or_else(|| line.strip_prefix("Name:"))
            })
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| self.id.clone())
    }

    /// Check if this agent can handle a specific task type
    pub fn can_handle(&self, task_type: &str) -> bool {
        let content = format!("{} {} {}", self.soul, self.identity, self.bootstrap);
        let keywords: Vec<&str> = match task_type {
            "code" => vec!["code", "program", "develop", "software", "debug"],
            "review" => vec!["review", "audit", "check", "analyze"],
            "write" => vec!["write", "document", "compose"],
            "research" => vec!["research", "investigate", "study"],
            "lead" => vec!["lead", "manage", "coordinate", "architect"],
            _ => vec![task_type],
        };

        let content_lower = content.to_lowercase();
        keywords.iter().any(|kw| content_lower.contains(kw))
    }
}

/// Agent Registry for discovered personalities
#[derive(Debug, Default)]
pub struct AgentRegistry {
    /// Registered agent personalities
    personalities: HashMap<String, AgentPersonality>,
    /// Whether agents have been discovered
    discovered: bool,
}

impl AgentRegistry {
    /// Create new empty registry
    pub fn new() -> Self {
        Self {
            personalities: HashMap::new(),
            discovered: false,
        }
    }

    /// Discover agents from the agents/ directory
    pub async fn discover(&mut self) -> crate::Result<usize> {
        let agents_dir = dirs::agents_dir();

        if !agents_dir.exists() {
            info!("Agents directory does not exist: {:?}", agents_dir);
            return Ok(0);
        }

        info!("Discovering agents from: {:?}", agents_dir);

        let mut count = 0;
        let mut entries = fs::read_dir(&agents_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Skip non-directories
            if !path.is_dir() {
                continue;
            }

            // Load personality
            match AgentPersonality::load(&path).await {
                Ok(personality) => {
                    if personality.is_valid {
                        self.personalities
                            .insert(personality.id.clone(), personality);
                        count += 1;
                    }
                }
                Err(e) => {
                    warn!("Failed to load agent from {:?}: {}", path, e);
                }
            }
        }

        self.discovered = true;
        info!("Discovered {} valid agents", count);

        // List discovered agents
        if count > 0 {
            debug!("Discovered agents:");
            for (id, personality) in &self.personalities {
                debug!("  - {} ({})", id, personality.display_name());
            }
        }

        Ok(count)
    }

    /// Get a personality by ID
    pub fn get(&self, id: &str) -> Option<&AgentPersonality> {
        self.personalities.get(id)
    }

    /// Get all personality IDs
    pub fn list(&self) -> Vec<String> {
        self.personalities.keys().cloned().collect()
    }

    /// Check if a personality exists
    pub fn has(&self, id: &str) -> bool {
        self.personalities.contains_key(id)
    }

    /// Get number of registered personalities
    pub fn len(&self) -> usize {
        self.personalities.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.personalities.is_empty()
    }

    /// Check if discovery has been run
    pub fn is_discovered(&self) -> bool {
        self.discovered
    }

    /// Find the best agent for a task
    pub fn find_for_task(&self, task_type: &str) -> Option<&AgentPersonality> {
        self.personalities
            .values()
            .find(|p| p.can_handle(task_type))
    }

    /// Get all personalities that can handle a task
    pub fn find_all_for_task(&self, task_type: &str) -> Vec<&AgentPersonality> {
        self.personalities
            .values()
            .filter(|p| p.can_handle(task_type))
            .collect()
    }

    /// Iterate over all personalities
    pub fn iter(&self) -> impl Iterator<Item = &AgentPersonality> {
        self.personalities.values()
    }
}

/// Global registry (can be stored in GatewayState)
pub type SharedAgentRegistry = std::sync::Arc<tokio::sync::RwLock<AgentRegistry>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_personality_builds_system_prompt() {
        let personality = AgentPersonality {
            id: "test".to_string(),
            soul: "You are helpful.".to_string(),
            identity: "# Test Agent\nI am a test.".to_string(),
            bootstrap: "Start by greeting.".to_string(),
            ..Default::default()
        };

        let prompt = personality.build_system_prompt();
        assert!(prompt.contains("Bootstrap"));
        assert!(prompt.contains("Identity"));
        assert!(prompt.contains("Soul"));
    }

    #[test]
    fn test_display_name_extraction() {
        let personality = AgentPersonality {
            id: "test-agent".to_string(),
            identity: "# My Agent Name\nDescription here.".to_string(),
            ..Default::default()
        };

        assert_eq!(personality.display_name(), "My Agent Name");
    }

    #[test]
    fn test_task_matching() {
        let personality = AgentPersonality {
            id: "coder".to_string(),
            soul: "I write code and debug software.".to_string(),
            ..Default::default()
        };

        assert!(personality.can_handle("code"));
        assert!(personality.can_handle("debug"));
    }
}
