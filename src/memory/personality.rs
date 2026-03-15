//! OpenClaw-Style Memory Architecture for Manta
//!
//! This module implements an OpenClaw-compatible memory system:
//! - SOUL.md: Core personality, values, behavioral guidelines
//! - IDENTITY.md: Agent identity, name, role definition
//! - BOOTSTRAP.md: Initial startup behavior, first-run logic
//! - USER.md: User-specific memory, preferences, conversation history
//! - AGENTS.md: Operating instructions and agent "memory"
//! - TOOLS.md: User-maintained tool notes and conventions
//!
//! All stored as markdown files with bounded size (default 4KB each).

use crate::error::MantaError;
use std::path::PathBuf;
use tokio::fs;
use tracing::{debug, info, warn};

/// Default maximum size for memory files (4KB)
pub const DEFAULT_MAX_MEMORY_SIZE: usize = 4096;

/// Types of OpenClaw-style memory
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// Soul memory - core personality, values, behavioral guidelines
    Soul,
    /// Identity memory - agent identity, name, role definition
    Identity,
    /// Bootstrap memory - initial startup behavior, first-run logic
    Bootstrap,
    /// User memory - user-specific data, preferences, conversation history
    User,
    /// Agents memory - operating instructions and agent "memory"
    Agents,
    /// Tools memory - user-maintained tool notes and conventions
    Tools,
}

impl MemoryType {
    /// Get the filename for this memory type
    pub fn filename(&self) -> &'static str {
        match self {
            MemoryType::Soul => "SOUL.md",
            MemoryType::Identity => "IDENTITY.md",
            MemoryType::Bootstrap => "BOOTSTRAP.md",
            MemoryType::User => "USER.md",
            MemoryType::Agents => "AGENTS.md",
            MemoryType::Tools => "TOOLS.md",
        }
    }

    /// Get the description of this memory type
    pub fn description(&self) -> &'static str {
        match self {
            MemoryType::Soul => "Core personality, values, behavioral guidelines, and character traits",
            MemoryType::Identity => "Agent identity, name, role definition, and self-concept",
            MemoryType::Bootstrap => "Initial startup behavior, first-run logic, and onboarding",
            MemoryType::User => "User-specific memory, preferences, conversation history, and learned context",
            MemoryType::Agents => "Operating instructions and agent memory for task execution",
            MemoryType::Tools => "User-maintained tool notes, conventions, and usage patterns",
        }
    }
}

/// Personality memory storage manager
#[derive(Debug, Clone)]
pub struct PersonalityMemory {
    /// Base directory for memory files
    base_dir: PathBuf,
    /// Maximum size for each memory file
    max_size: usize,
}

impl PersonalityMemory {
    /// Create a new personality memory manager
    ///
    /// Uses tiered lookup like OpenClaw:
    /// 1. Workspace level: <workspace>/.manta/memory/ (if in a workspace)
    /// 2. User level: ~/.manta/memory-files/ (fallback)
    pub async fn new() -> crate::Result<Self> {
        // Try workspace level first
        if let Some(workspace_dir) = Self::find_workspace_memory_dir() {
            if workspace_dir.exists() {
                tracing::info!("Using workspace-level personality memory: {:?}", workspace_dir);
                return Self::with_dir(workspace_dir).await;
            }
        }

        // Fall back to user level
        let base_dir = crate::dirs::workspace_memory_dir();
        tracing::info!("Using user-level personality memory: {:?}", base_dir);
        Self::with_dir(base_dir).await
    }

    /// Find workspace-level memory directory
    fn find_workspace_memory_dir() -> Option<PathBuf> {
        // Look for workspace root marker
        let cwd = std::env::current_dir().ok()?;
        let mut current = cwd.as_path();

        loop {
            // Check for workspace markers
            let markers = [".manta-workspace", ".git", "manta.workspace.toml"];
            for marker in &markers {
                if current.join(marker).exists() {
                    let memory_dir = current.join(".manta").join("memory");
                    return Some(memory_dir);
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

    /// Create a dual memory manager with specific directory
    pub async fn with_dir(base_dir: PathBuf) -> crate::Result<Self> {
        // Ensure directory exists
        fs::create_dir_all(&base_dir).await.map_err(|e| {
            MantaError::Storage {
                context: format!("Failed to create directory: {:?}", base_dir),
                details: e.to_string(),
            }
        })?;

        Ok(Self {
            base_dir,
            max_size: DEFAULT_MAX_MEMORY_SIZE,
        })
    }

    /// Set maximum memory size
    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size = max_size;
        self
    }

    /// Get the path for a specific memory type
    fn memory_path(&self, mem_type: MemoryType) -> PathBuf {
        self.base_dir.join(mem_type.filename())
    }

    /// Read memory content
    pub async fn read(&self, mem_type: MemoryType) -> crate::Result<String> {
        let path = self.memory_path(mem_type);

        if !path.exists() {
            debug!("Memory file {:?} does not exist, returning empty", mem_type);
            return Ok(String::new());
        }

        let content = fs::read_to_string(&path).await.map_err(|e| {
            MantaError::Storage {
                context: format!("Failed to read memory file: {:?}", path),
                details: e.to_string(),
            }
        })?;

        debug!("Read {} bytes from {:?}", content.len(), mem_type);
        Ok(content)
    }

    /// Write memory content (with size limit)
    pub async fn write(&self, mem_type: MemoryType, content: &str) -> crate::Result<()> {
        let _path = self.memory_path(mem_type);

        // Check size limit
        if content.len() > self.max_size {
            warn!(
                "Memory content exceeds max size ({} > {}), truncating",
                content.len(),
                self.max_size
            );
            let truncated: String = content.chars().take(self.max_size).collect();
            return self.write_unchecked(mem_type, &truncated).await;
        }

        // Security scan for injection patterns
        if let Some(threat) = self.scan_for_threats(content) {
            warn!("Security threat detected in memory: {}", threat);
            return Err(MantaError::Validation(format!(
                "Security threat detected in memory: {}",
                threat
            )));
        }

        self.write_unchecked(mem_type, content).await
    }

    /// Write without checks (internal use)
    async fn write_unchecked(&self, mem_type: MemoryType, content: &str) -> crate::Result<()> {
        let path = self.memory_path(mem_type);

        fs::write(&path, content).await.map_err(|e| {
            MantaError::Storage {
                context: format!("Failed to write memory file: {:?}", path),
                details: e.to_string(),
            }
        })?;

        info!("Wrote {} bytes to {:?}", content.len(), mem_type);
        Ok(())
    }

    /// Append to memory content (with size limit)
    pub async fn append(&self, mem_type: MemoryType, addition: &str) -> crate::Result<()> {
        let current = self.read(mem_type).await?;
        let new_content = format!("{}\n{}", current, addition);
        self.write(mem_type, &new_content).await
    }

    /// Check if memory exists
    pub async fn exists(&self, mem_type: MemoryType) -> bool {
        self.memory_path(mem_type).exists()
    }

    /// Get memory size in bytes
    pub async fn size(&self, mem_type: MemoryType) -> crate::Result<usize> {
        let content = self.read(mem_type).await?;
        Ok(content.len())
    }

    /// Clear memory
    pub async fn clear(&self, mem_type: MemoryType) -> crate::Result<()> {
        self.write_unchecked(mem_type, "").await
    }

    /// Get memory content formatted for system prompt
    pub async fn format_for_prompt(&self) -> crate::Result<String> {
        // OpenClaw-style personality files (loaded in priority order)
        // AGENTS.md and TOOLS.md are loaded first as they provide operating instructions
        let agents = self.read(MemoryType::Agents).await?;
        let tools = self.read(MemoryType::Tools).await?;
        let identity = self.read(MemoryType::Identity).await?;
        let soul = self.read(MemoryType::Soul).await?;
        let bootstrap = self.read(MemoryType::Bootstrap).await?;
        let user = self.read(MemoryType::User).await?;

        let mut sections = Vec::new();

        // AGENTS.md - Operating instructions (highest priority after system)
        if !agents.is_empty() {
            sections.push(format!("## Agents\n{}\n", agents.trim()));
        }

        // TOOLS.md - Tool conventions and notes
        if !tools.is_empty() {
            sections.push(format!("## Tools\n{}\n", tools.trim()));
        }

        if !identity.is_empty() {
            sections.push(format!("## Identity\n{}\n", identity.trim()));
        }

        if !soul.is_empty() {
            sections.push(format!("## Soul\n{}\n", soul.trim()));
        }

        if !bootstrap.is_empty() {
            sections.push(format!("## Bootstrap\n{}\n", bootstrap.trim()));
        }

        if !user.is_empty() {
            sections.push(format!("## User\n{}\n", user.trim()));
        }

        if sections.is_empty() {
            Ok(String::new())
        } else {
            Ok(format!("\n### Learned Context\n{}\n", sections.join("\n")))
        }
    }

    /// Scan content for security threats
    fn scan_for_threats(&self, content: &str) -> Option<String> {
        // List of suspicious patterns
        let patterns = [
            ("system_prompt_injection", r"(?i)(system|assistant|user)\s*:\s*"),
            ("command_injection", r"(?i)(;|\|\||&&|`|<\(|>\$)\s*[a-z]+",),
            ("path_traversal", r"\.\./|\.\.\\"),
            ("exfiltration", r"(?i)(curl|wget|fetch)\s+.*http"),
        ];

        for (name, pattern) in patterns {
            if let Ok(re) = regex::Regex::new(pattern) {
                if re.is_match(content) {
                    return Some(name.to_string());
                }
            }
        }

        None
    }

    /// Initialize default memory files if they don't exist
    pub async fn initialize_defaults(&self) -> crate::Result<()> {
        // IDENTITY.md - Agent identity (OpenClaw-style)
        if !self.exists(MemoryType::Identity).await {
            let default_identity = r#"# IDENTITY

Agent identity and self-concept.

## Name
Manta

## Role
AI Assistant with software engineering expertise

## Capabilities
- Code analysis and generation
- File and system operations
- Tool use and orchestration
- Memory and learning

## Purpose
Help users accomplish tasks efficiently while respecting their preferences and constraints.
"#;
            self.write(MemoryType::Identity, default_identity).await?;
        }

        // SOUL.md - Core personality (OpenClaw-style)
        if !self.exists(MemoryType::Soul).await {
            let default_soul = r#"# SOUL

Core personality, values, and behavioral guidelines.

## Values
- Be helpful, harmless, and honest
- Respect user autonomy and privacy
- Prioritize clarity over cleverness

## Communication Style
- Clear and concise explanations
- Ask clarifying questions when uncertain
- Admit limitations openly

## Behavioral Guidelines
- Always confirm destructive operations
- Provide alternatives when saying no
- Learn from user corrections
"#;
            self.write(MemoryType::Soul, default_soul).await?;
        }

        // BOOTSTRAP.md - Initial behavior (OpenClaw-style)
        if !self.exists(MemoryType::Bootstrap).await {
            let default_bootstrap = r#"# BOOTSTRAP

Initial startup behavior and first-run logic.

## Greeting Style
- Friendly but professional
- Brief status summary when relevant
- Offer assistance without being pushy

## First Run Behavior
- Introduce capabilities concisely
- Ask about user preferences
- Set up initial context

## Session Start
- Review recent context if available
- Confirm current task or goals
- Check for pending items
"#;
            self.write(MemoryType::Bootstrap, default_bootstrap).await?;
        }

        Ok(())
    }
}

/// Tool for managing personality memory
pub mod tool {
    use super::*;
    use crate::tools::{Tool, ToolContext, ToolExecutionResult};
    use async_trait::async_trait;
    use serde_json::json;

    /// Tool for reading and writing personality memory
    #[derive(Debug)]
    pub struct PersonalityMemoryTool {
        memory: PersonalityMemory,
    }

    impl PersonalityMemoryTool {
        /// Create a new personality memory tool
        pub async fn new() -> crate::Result<Self> {
            let memory = PersonalityMemory::new().await?;
            Ok(Self { memory })
        }

        /// Create with custom directory
        pub async fn with_dir(dir: PathBuf) -> crate::Result<Self> {
            let memory = PersonalityMemory::with_dir(dir).await?;
            Ok(Self { memory })
        }
    }

    #[async_trait]
    impl Tool for PersonalityMemoryTool {
        fn name(&self) -> &str {
            "personality_memory"
        }

        fn description(&self) -> &str {
            r#"Read and write to the agent's OpenClaw-style memory system.

This tool manages personality and identity memory files:
- identity: Agent identity, name, role definition (IDENTITY.md)
- soul: Core personality, values, behavioral guidelines (SOUL.md)
- bootstrap: Initial startup behavior, first-run logic (BOOTSTRAP.md)

Use this to define agent personality and behavior across sessions.
These files are loaded into the system prompt at startup."#
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["read", "write", "append", "clear"],
                        "description": "The action to perform"
                    },
                    "memory_type": {
                        "type": "string",
                        "enum": ["identity", "soul", "bootstrap"],
                        "description": "Which memory file to access"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write (for write/append actions)"
                    }
                },
                "required": ["action", "memory_type"]
            })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
            _context: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            let action = args["action"]
                .as_str()
                .ok_or_else(|| MantaError::Validation("action is required".to_string()))?;

            let mem_type_str = args["memory_type"]
                .as_str()
                .ok_or_else(|| MantaError::Validation("memory_type is required".to_string()))?;

            let mem_type = match mem_type_str {
                "identity" => MemoryType::Identity,
                "soul" => MemoryType::Soul,
                "bootstrap" => MemoryType::Bootstrap,
                _ => {
                    return Err(MantaError::Validation(format!(
                        "Invalid memory_type: {}",
                        mem_type_str
                    )))
                }
            };

            match action {
                "read" => {
                    let content = self.memory.read(mem_type).await?;
                    Ok(ToolExecutionResult::success(format!(
                        "Memory content:\n{}",
                        content
                    ))
                    .with_data(json!({
                        "memory_type": mem_type_str,
                        "content": content,
                        "size": content.len()
                    })))
                }

                "write" => {
                    let content = args["content"]
                        .as_str()
                        .ok_or_else(|| MantaError::Validation(
                            "content is required for write action".to_string()
                        ))?;

                    self.memory.write(mem_type, content).await?;
                    Ok(ToolExecutionResult::success(format!(
                        "Wrote {} bytes to {:?}",
                        content.len(),
                        mem_type
                    ))
                    .with_data(json!({
                        "memory_type": mem_type_str,
                        "bytes_written": content.len()
                    })))
                }

                "append" => {
                    let content = args["content"]
                        .as_str()
                        .ok_or_else(|| MantaError::Validation(
                            "content is required for append action".to_string()
                        ))?;

                    self.memory.append(mem_type, content).await?;
                    Ok(ToolExecutionResult::success(format!(
                        "Appended to {:?}",
                        mem_type
                    ))
                    .with_data(json!({"memory_type": mem_type_str})))
                }

                "clear" => {
                    self.memory.clear(mem_type).await?;
                    Ok(ToolExecutionResult::success(format!(
                        "Cleared {:?}",
                        mem_type
                    ))
                    .with_data(json!({"memory_type": mem_type_str})))
                }

                _ => Err(MantaError::Validation(format!(
                    "Unknown action: {}",
                    action
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_personality_memory_read_write() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = PersonalityMemory::with_dir(temp_dir.clone())
            .await
            .unwrap();

        // Write to identity memory
        memory
            .write(MemoryType::Identity, "Test content")
            .await
            .unwrap();

        // Read it back
        let content = memory.read(MemoryType::Identity).await.unwrap();
        assert_eq!(content, "Test content");
    }

    #[tokio::test]
    async fn test_personality_memory_size_limit() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = PersonalityMemory::with_dir(temp_dir.clone())
            .await
            .unwrap()
            .with_max_size(10);

        // Write content larger than limit
        memory
            .write(MemoryType::Soul, "This is a long content")
            .await
            .unwrap();

        // Should be truncated
        let content = memory.read(MemoryType::Soul).await.unwrap();
        assert_eq!(content.len(), 10);
    }

    #[tokio::test]
    async fn test_personality_memory_exists() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = PersonalityMemory::with_dir(temp_dir.clone())
            .await
            .unwrap();

        assert!(!memory.exists(MemoryType::Bootstrap).await);

        memory
            .write(MemoryType::Bootstrap, "content")
            .await
            .unwrap();

        assert!(memory.exists(MemoryType::Bootstrap).await);
    }
}
