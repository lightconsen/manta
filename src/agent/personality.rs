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
use tracing::{debug, info, warn};

use crate::agent::AgentConfig;
use crate::dirs;

/// Maximum size for personality files (4KB default)
const DEFAULT_MAX_FILE_SIZE: usize = 4096;

/// Controls which personality files are included in the system prompt.
///
/// `Primary` produces the full prompt (Bootstrap + Identity + Soul + Agents + Tools).
/// `Subagent` omits Bootstrap and User — these contain startup-only instructions
/// that are irrelevant (and wasteful) for spawned subagents and cron jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonalityContext {
    /// Full prompt for the primary interactive session.
    Primary,
    /// Reduced prompt for spawned subagents and cron jobs.
    Subagent,
}

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
    /// HEARTBEAT.md - Periodic task checklist and proactive work reminders
    pub heartbeat: String,
    /// MEMORY.md - Curated long-term memory (personal context)
    pub memory: String,
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
        personality.heartbeat = personality.load_file("HEARTBEAT.md").await;
        personality.memory = personality.load_file("MEMORY.md").await;

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

    /// Convert personality to AgentConfig using the full (Primary) prompt.
    pub fn to_agent_config(&self) -> AgentConfig {
        self.to_agent_config_for(PersonalityContext::Primary)
    }

    /// Convert personality to AgentConfig for the given context.
    ///
    /// Use [`PersonalityContext::Subagent`] when spawning child agents or cron
    /// jobs to omit startup-only sections (Bootstrap, User) and reduce token
    /// usage.
    pub fn to_agent_config_for(&self, ctx: PersonalityContext) -> AgentConfig {
        let system_prompt = match ctx {
            PersonalityContext::Primary => self.build_system_prompt(),
            PersonalityContext::Subagent => self.build_subagent_prompt(),
        };

        // Inject agent identity so the agent knows its own ID and can manage its files
        let system_prompt = format!(
            "{}\n\n## Agent Identity\n\nYour agent ID is: `{}`\nYour agent directory is: `{}`\nYou may edit files in your agent directory (including HEARTBEAT.md) to manage your personality and periodic tasks when explicitly asked by the user.",
            system_prompt,
            self.id,
            self.path.display()
        );

        AgentConfig {
            system_prompt,
            max_context_tokens: 4096,
            max_concurrent_tools: 5,
            temperature: 0.7,
            max_tokens: 2048,
            skills_prompt: None,
            max_turns: None,
            compaction_model: None,
            workspace_dir: None,
            workspace_only: false,
            heartbeat: None,
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

        // HEARTBEAT.md - Periodic tasks and proactive work
        if !self.heartbeat.is_empty() {
            sections.push(format!("## Heartbeat\n{}\n", self.heartbeat.trim()));
        }

        // MEMORY.md - Curated long-term memory (personal context)
        if !self.memory.is_empty() {
            sections.push(format!("## Memory\n{}\n", self.memory.trim()));
        }

        if sections.is_empty() {
            // Fallback to default
            AgentConfig::default().system_prompt
        } else {
            sections.join("\n")
        }
    }

    /// Build a reduced system prompt for subagents and cron jobs.
    ///
    /// Includes: Identity, Soul, Agents, Tools, User.
    /// Excludes: Bootstrap (startup-only), Heartbeat (periodic tasks), Memory (personal context).
    fn build_subagent_prompt(&self) -> String {
        let mut sections = Vec::new();

        if !self.identity.is_empty() {
            sections.push(format!("## Identity\n{}\n", self.identity.trim()));
        }

        if !self.soul.is_empty() {
            sections.push(format!("## Soul\n{}\n", self.soul.trim()));
        }

        if !self.agents.is_empty() {
            sections.push(format!("## Agents\n{}\n", self.agents.trim()));
        }

        if !self.tools.is_empty() {
            sections.push(format!("## Tools\n{}\n", self.tools.trim()));
        }

        if !self.user.is_empty() {
            sections.push(format!("## User\n{}\n", self.user.trim()));
        }

        // Explicitly excluded: bootstrap, heartbeat, memory
        // - Bootstrap: startup-only instructions irrelevant to subagents
        // - Heartbeat: periodic task checklist for main session only
        // - Memory: contains personal context that shouldn't leak to strangers

        if sections.is_empty() {
            AgentConfig::default().system_prompt
        } else {
            sections.join("\n")
        }
    }

    /// Get the agent's display name from identity
    pub fn display_name(&self) -> String {
        let lines: Vec<&str> = self.identity.lines().collect();

        // 1. Try structured format:
        //    # Agent Identity
        //    ## name
        //    小王
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.eq_ignore_ascii_case("## name")
                || trimmed.eq_ignore_ascii_case("##name")
                || trimmed.eq_ignore_ascii_case("name:")
            {
                if let Some(val) = lines.get(i + 1) {
                    let name = val.trim();
                    if !name.is_empty() {
                        return name.to_string();
                    }
                }
            }
        }

        // 2. Fallback to first heading line
        lines
            .first()
            .and_then(|line| {
                let trimmed = line.trim();
                // # Title  →  "Title"
                trimmed.strip_prefix("#").map(|s| s.trim().to_string())
            })
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

    /// Get all possible aliases for this agent.
    ///
    /// Includes the display name, the agent ID, and short forms derived from both.
    /// Example: "secretary-xiaowang" with display name "秘书小王" produces
    /// `["secretary-xiaowang", "xiaowang", "秘书小王", "小王"]`.
    pub fn aliases(&self) -> Vec<String> {
        let mut aliases = Vec::new();

        // Agent ID (always included)
        aliases.push(self.id.clone());

        // Short form from ID: "secretary-xiaowang" -> "xiaowang"
        if let Some(short) = self.id.rsplit('-').next() {
            if short != self.id {
                aliases.push(short.to_string());
            }
        }

        // Display name from IDENTITY.md
        let display = self.display_name();
        if !display.is_empty() && display != self.id {
            aliases.push(display.clone());
            // Extract short nicknames from display name:
            // "秘书小王" -> "小王"
            // "My Agent Name" -> "My", "Agent", "Name", "Agent Name"
            for word in display.split_whitespace() {
                let trimmed = word.trim();
                if trimmed.len() >= 2 && !aliases.iter().any(|a| a == trimmed) {
                    aliases.push(trimmed.to_string());
                }
            }
            // Also try last 2-4 chars as a common nickname pattern (Chinese)
            if display.chars().count() >= 3 {
                let suffix: String = display
                    .chars()
                    .rev()
                    .take(2)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                if suffix.len() >= 2 && !aliases.iter().any(|a| a == &suffix) {
                    aliases.push(suffix);
                }
            }
        }

        aliases
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
                        // Ensure agent subdirectories exist (workspace/, data/)
                        let agent_id = &personality.id;
                        let workspace_dir = dirs::agent_workspace_dir(agent_id);
                        let data_dir = dirs::agent_data_dir(agent_id);
                        for dir in [&workspace_dir, &data_dir] {
                            if let Err(e) = tokio::fs::create_dir_all(dir).await {
                                warn!("Failed to create agent directory {:?}: {}", dir, e);
                            }
                        }
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

    /// Find an agent whose aliases match the given name.
    ///
    /// Matches exact alias strings (case-insensitive).  Returns the first
    /// matching personality and the matched alias text so the caller can
    /// strip it from the original message.
    pub fn find_by_alias(&self, name: &str) -> Option<(&AgentPersonality, String)> {
        let name_lower = name.to_lowercase();
        for personality in self.personalities.values() {
            for alias in personality.aliases() {
                if alias.to_lowercase() == name_lower {
                    return Some((personality, alias));
                }
            }
        }
        None
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

    #[test]
    fn test_personality_context_primary_includes_bootstrap() {
        let personality = AgentPersonality {
            id: "agent".to_string(),
            bootstrap: "Start by greeting.".to_string(),
            identity: "I am an agent.".to_string(),
            soul: "Be helpful.".to_string(),
            user: "User prefers terse replies.".to_string(),
            agents: "Work with other agents.".to_string(),
            tools: "Use tools wisely.".to_string(),
            heartbeat: "Check inbox every hour.".to_string(),
            memory: "User likes coffee.".to_string(),
            ..Default::default()
        };

        let config = personality.to_agent_config_for(PersonalityContext::Primary);
        assert!(config.system_prompt.contains("Bootstrap"), "Primary should include Bootstrap");
        assert!(config.system_prompt.contains("Identity"));
        assert!(config.system_prompt.contains("Soul"));
        assert!(config.system_prompt.contains("Heartbeat"));
        assert!(config.system_prompt.contains("Memory"));
    }

    #[test]
    fn test_personality_context_subagent_excludes_bootstrap_heartbeat_and_memory() {
        let personality = AgentPersonality {
            id: "agent".to_string(),
            bootstrap: "Start by greeting.".to_string(),
            identity: "I am an agent.".to_string(),
            soul: "Be helpful.".to_string(),
            user: "User prefers terse replies.".to_string(),
            agents: "Work with other agents.".to_string(),
            tools: "Use tools wisely.".to_string(),
            heartbeat: "Check inbox every hour.".to_string(),
            memory: "User likes coffee.".to_string(),
            ..Default::default()
        };

        let config = personality.to_agent_config_for(PersonalityContext::Subagent);
        // Excluded: Bootstrap (startup-only), Heartbeat (periodic tasks), Memory (personal context)
        assert!(
            !config.system_prompt.contains("Bootstrap"),
            "Subagent should NOT include Bootstrap"
        );
        assert!(
            !config.system_prompt.contains("Heartbeat"),
            "Subagent should NOT include Heartbeat"
        );
        assert!(
            !config.system_prompt.contains("User likes coffee"),
            "Subagent should NOT include Memory section content"
        );
        // Included: Identity, Soul, Agents, Tools, User
        assert!(config.system_prompt.contains("Identity"));
        assert!(config.system_prompt.contains("Soul"));
        assert!(config.system_prompt.contains("Agents"));
        assert!(config.system_prompt.contains("Tools"));
        assert!(config.system_prompt.contains("User prefers terse"));
    }

    #[test]
    fn test_to_agent_config_delegates_to_primary() {
        let personality = AgentPersonality {
            id: "agent".to_string(),
            bootstrap: "Boot!".to_string(),
            soul: "Be nice.".to_string(),
            ..Default::default()
        };

        let default_cfg = personality.to_agent_config();
        let primary_cfg = personality.to_agent_config_for(PersonalityContext::Primary);
        assert_eq!(default_cfg.system_prompt, primary_cfg.system_prompt);
    }

    #[test]
    fn test_subagent_prompt_fallback_when_all_empty() {
        let personality = AgentPersonality {
            id: "empty".to_string(),
            ..Default::default()
        };

        let config = personality.to_agent_config_for(PersonalityContext::Subagent);
        // Should not panic and should return the default system prompt
        assert!(!config.system_prompt.is_empty());
    }

    #[test]
    fn test_aliases_from_id_and_display_name() {
        let personality = AgentPersonality {
            id: "secretary-xiaowang".to_string(),
            identity: "# 秘书小王\n私人秘书".to_string(),
            ..Default::default()
        };

        let aliases = personality.aliases();
        assert!(aliases.contains(&"secretary-xiaowang".to_string()));
        assert!(aliases.contains(&"xiaowang".to_string()));
        assert!(aliases.contains(&"秘书小王".to_string()));
        assert!(aliases.contains(&"小王".to_string()));
    }

    #[test]
    fn test_find_by_alias_exact_match() {
        let mut registry = AgentRegistry::new();
        registry.personalities.insert(
            "secretary-xiaowang".to_string(),
            AgentPersonality {
                id: "secretary-xiaowang".to_string(),
                identity: "# 秘书小王\n私人秘书".to_string(),
                ..Default::default()
            },
        );

        let (p, alias) = registry.find_by_alias("小王").unwrap();
        assert_eq!(p.id, "secretary-xiaowang");
        assert_eq!(alias, "小王");

        let (p2, _) = registry.find_by_alias("xiaowang").unwrap();
        assert_eq!(p2.id, "secretary-xiaowang");
    }

    #[test]
    fn test_find_by_alias_case_insensitive() {
        let mut registry = AgentRegistry::new();
        registry.personalities.insert(
            "coder".to_string(),
            AgentPersonality {
                id: "coder".to_string(),
                identity: "# Code Assistant\nI write code.".to_string(),
                ..Default::default()
            },
        );

        let (p, _) = registry.find_by_alias("CODE ASSISTANT").unwrap();
        assert_eq!(p.id, "coder");
    }

    #[test]
    fn test_find_by_alias_no_match() {
        let registry = AgentRegistry::new();
        assert!(registry.find_by_alias("nonexistent").is_none());
    }
}
