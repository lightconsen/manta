//! Dual Memory Architecture for Manta
//!
//! This module implements a dual memory system inspired by Hermes-Agent:
//! - Procedural Memory: Environment facts, tool quirks, conventions
//! - User Model: Preferences, communication style, habits
//!
//! Both are stored as markdown files with bounded size (default 4KB each).

use crate::error::MantaError;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info, warn};

/// Default maximum size for memory files (4KB)
pub const DEFAULT_MAX_MEMORY_SIZE: usize = 4096;

/// Types of dual memory
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DualMemoryType {
    /// Procedural memory - environment facts, tool quirks, conventions
    Procedural,
    /// User model - preferences, communication style, habits
    UserModel,
}

impl DualMemoryType {
    /// Get the filename for this memory type
    pub fn filename(&self) -> &'static str {
        match self {
            DualMemoryType::Procedural => "agent.md",
            DualMemoryType::UserModel => "user.md",
        }
    }

    /// Get the description of this memory type
    pub fn description(&self) -> &'static str {
        match self {
            DualMemoryType::Procedural => {
                "Environment facts, tool quirks, conventions, and learned behaviors"
            }
            DualMemoryType::UserModel => {
                "User preferences, communication style, habits, and personal information"
            }
        }
    }
}

/// Dual memory storage manager
#[derive(Debug, Clone)]
pub struct DualMemory {
    /// Base directory for memory files
    base_dir: PathBuf,
    /// Maximum size for each memory file
    max_size: usize,
}

impl DualMemory {
    /// Create a new dual memory manager with default location
    pub async fn new() -> crate::Result<Self> {
        let base_dir = dirs::config_dir()
            .ok_or_else(|| MantaError::Internal("Could not find config directory".to_string()))?
            .join("manta")
            .join("memory");

        Self::with_dir(base_dir).await
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
    fn memory_path(&self, mem_type: DualMemoryType) -> PathBuf {
        self.base_dir.join(mem_type.filename())
    }

    /// Read memory content
    pub async fn read(&self, mem_type: DualMemoryType) -> crate::Result<String> {
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
    pub async fn write(&self, mem_type: DualMemoryType, content: &str) -> crate::Result<()> {
        let path = self.memory_path(mem_type);

        // Check size limit
        if content.len() > self.max_size {
            warn!(
                "Memory content exceeds max size ({} > {}), truncating",
                content.len(),
                self.max_size
            );
            let truncated = &content[..self.max_size];
            return self.write_unchecked(mem_type, truncated).await;
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
    async fn write_unchecked(&self, mem_type: DualMemoryType, content: &str) -> crate::Result<()> {
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
    pub async fn append(&self, mem_type: DualMemoryType, addition: &str) -> crate::Result<()> {
        let current = self.read(mem_type).await?;
        let new_content = format!("{}\n{}", current, addition);
        self.write(mem_type, &new_content).await
    }

    /// Check if memory exists
    pub async fn exists(&self, mem_type: DualMemoryType) -> bool {
        self.memory_path(mem_type).exists()
    }

    /// Get memory size in bytes
    pub async fn size(&self, mem_type: DualMemoryType) -> crate::Result<usize> {
        let content = self.read(mem_type).await?;
        Ok(content.len())
    }

    /// Clear memory
    pub async fn clear(&self, mem_type: DualMemoryType) -> crate::Result<()> {
        self.write_unchecked(mem_type, "").await
    }

    /// Get memory content formatted for system prompt
    pub async fn format_for_prompt(&self) -> crate::Result<String> {
        let procedural = self.read(DualMemoryType::Procedural).await?;
        let user_model = self.read(DualMemoryType::UserModel).await?;

        let mut sections = Vec::new();

        if !procedural.is_empty() {
            sections.push(format!(
                "## Procedural Memory\n{}\n",
                procedural.trim()
            ));
        }

        if !user_model.is_empty() {
            sections.push(format!(
                "## User Model\n{}\n",
                user_model.trim()
            ));
        }

        if sections.is_empty() {
            Ok(String::new())
        } else {
            Ok(format!(
                "\n### Learned Context\n{}\n",
                sections.join("\n")
            ))
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
        // Procedural memory default
        if !self.exists(DualMemoryType::Procedural).await {
            let default_procedural = r#"# Procedural Memory

This file contains learned facts about the environment and tools.

## Shell Commands
- Use `ls -la` for detailed directory listings
- Use `find` with `-name` for filename searches
- Prefer `grep -r` for recursive text search

## File Operations
- Always verify file existence before reading
- Use atomic writes for critical files
- Respect .gitignore patterns
"#;
            self.write(DualMemoryType::Procedural, default_procedural)
                .await?;
        }

        // User model default
        if !self.exists(DualMemoryType::UserModel).await {
            let default_user = r#"# User Model

This file contains information about the user's preferences.

## Communication Style
- Preference: concise and direct
- Technical level: advanced

## Common Tasks
- Code review and analysis
- File and system management
- Information retrieval
"#;
            self.write(DualMemoryType::UserModel, default_user).await?;
        }

        Ok(())
    }
}

/// Tool for managing dual memory
pub mod tool {
    use super::*;
    use crate::tools::{Tool, ToolContext, ToolExecutionResult};
    use async_trait::async_trait;
    use serde_json::json;

    /// Tool for reading and writing dual memory
    #[derive(Debug)]
    pub struct DualMemoryTool {
        memory: DualMemory,
    }

    impl DualMemoryTool {
        /// Create a new dual memory tool
        pub async fn new() -> crate::Result<Self> {
            let memory = DualMemory::new().await?;
            Ok(Self { memory })
        }

        /// Create with custom directory
        pub async fn with_dir(dir: PathBuf) -> crate::Result<Self> {
            let memory = DualMemory::with_dir(dir).await?;
            Ok(Self { memory })
        }
    }

    #[async_trait]
    impl Tool for DualMemoryTool {
        fn name(&self) -> &str {
            "dual_memory"
        }

        fn description(&self) -> &str {
            r#"Read and write to the agent's dual memory system.

This tool manages two types of persistent memory:
- procedural: Environment facts, tool quirks, conventions
- user_model: User preferences, communication style, habits

Use this to remember important information across sessions.
The agent can read from memory at startup and write updates as it learns."#
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
                        "enum": ["procedural", "user_model"],
                        "description": "Which memory to access"
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
                "procedural" => DualMemoryType::Procedural,
                "user_model" => DualMemoryType::UserModel,
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
    async fn test_dual_memory_read_write() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = DualMemory::with_dir(temp_dir.clone())
            .await
            .unwrap();

        // Write to procedural memory
        memory
            .write(DualMemoryType::Procedural, "Test content")
            .await
            .unwrap();

        // Read it back
        let content = memory.read(DualMemoryType::Procedural).await.unwrap();
        assert_eq!(content, "Test content");
    }

    #[tokio::test]
    async fn test_dual_memory_size_limit() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = DualMemory::with_dir(temp_dir.clone())
            .await
            .unwrap()
            .with_max_size(10);

        // Write content larger than limit
        memory
            .write(DualMemoryType::Procedural, "This is a long content")
            .await
            .unwrap();

        // Should be truncated
        let content = memory.read(DualMemoryType::Procedural).await.unwrap();
        assert_eq!(content.len(), 10);
    }

    #[tokio::test]
    async fn test_dual_memory_exists() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = DualMemory::with_dir(temp_dir.clone())
            .await
            .unwrap();

        assert!(!memory.exists(DualMemoryType::Procedural).await);

        memory
            .write(DualMemoryType::Procedural, "content")
            .await
            .unwrap();

        assert!(memory.exists(DualMemoryType::Procedural).await);
    }
}
