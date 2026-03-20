//! OpenClaw-Style Memory Architecture for Manta
//!
//! This module implements an OpenClaw-compatible memory system:
//! - SOUL.md: Core personality, values, behavioral guidelines
//! - IDENTITY.md: Agent identity, name, role definition
//! - BOOTSTRAP.md: Initial startup behavior, first-run logic
//! - USER.md: User-specific memory, preferences, conversation history
//! - AGENTS.md: Operating instructions and agent "memory"
//! - TOOLS.md: User-maintained tool notes and conventions
//! - memory/*.md: Dated/named memory fragments loaded dynamically
//!
//! Files are bounded per-file (default 20 KB) and in total (default 150 KB).
//! When a file exceeds the per-file cap, the first 70% and last 20% are kept
//! with a truncation marker between them.
//!
//! An mtime+size file cache avoids re-reading unchanged files on every turn.

use crate::error::MantaError;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Default maximum size for each personality memory file (20 KB).
pub const DEFAULT_MAX_MEMORY_SIZE: usize = 20_000;

/// Default total budget across all files (150 KB).
pub const DEFAULT_TOTAL_MAX_SIZE: usize = 150_000;

/// Truncate `content` to `max_chars`, keeping the first 70% and the last 20%
/// with a `[... N chars truncated ...]` marker in between.
///
/// If the content fits within `max_chars` it is returned unchanged.
pub fn truncate_with_head_tail(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let head = (max_chars as f64 * 0.70) as usize;
    let tail = (max_chars as f64 * 0.20) as usize;
    // Clamp to valid char boundaries.
    let head = head.min(content.len());
    let tail_start = content.len().saturating_sub(tail);
    let tail_start = tail_start.max(head);
    let truncated = content.len().saturating_sub(head).saturating_sub(tail);
    format!(
        "{}\n\n[... {} chars truncated ...]\n\n{}",
        &content[..head],
        truncated,
        &content[tail_start..]
    )
}

// ── File cache ────────────────────────────────────────────────────────────────

/// A cached view of a single file on disk.
#[derive(Clone, Debug)]
struct CachedFile {
    content: String,
    mtime: SystemTime,
    size: u64,
}

/// Thread-safe, mtime/size-invalidated cache of file contents.
type FileCache = Arc<RwLock<HashMap<PathBuf, CachedFile>>>;

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
            MemoryType::Soul => {
                "Core personality, values, behavioral guidelines, and character traits"
            }
            MemoryType::Identity => "Agent identity, name, role definition, and self-concept",
            MemoryType::Bootstrap => "Initial startup behavior, first-run logic, and onboarding",
            MemoryType::User => {
                "User-specific memory, preferences, conversation history, and learned context"
            }
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
    /// Maximum size for each individual memory file (chars)
    max_size: usize,
    /// Maximum combined size across all files loaded into a prompt (chars)
    total_max_size: usize,
    /// In-process file cache (invalidated on mtime/size change)
    cache: FileCache,
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
        fs::create_dir_all(&base_dir)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to create directory: {:?}", base_dir),
                details: e.to_string(),
            })?;

        Ok(Self {
            base_dir,
            max_size: DEFAULT_MAX_MEMORY_SIZE,
            total_max_size: DEFAULT_TOTAL_MAX_SIZE,
            cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Set the per-file character cap.
    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size = max_size;
        self
    }

    /// Set the total character budget across all files.
    pub fn with_total_max_size(mut self, total_max_size: usize) -> Self {
        self.total_max_size = total_max_size;
        self
    }

    /// Get the path for a specific memory type
    fn memory_path(&self, mem_type: MemoryType) -> PathBuf {
        self.base_dir.join(mem_type.filename())
    }

    /// Read a file from `path`, using the in-process cache when the file is
    /// unchanged (same mtime and size).
    async fn read_with_cache(&self, path: &Path) -> crate::Result<String> {
        if !path.exists() {
            return Ok(String::new());
        }

        // Try the cache first.
        if let Ok(meta) = fs::metadata(path).await {
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let size = meta.len();
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(path) {
                if cached.mtime == mtime && cached.size == size {
                    debug!("Cache hit for {:?}", path);
                    return Ok(cached.content.clone());
                }
            }
        }

        // Cache miss or stale — read from disk.
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to read file: {:?}", path),
                details: e.to_string(),
            })?;

        // Update the cache entry.
        if let Ok(meta) = fs::metadata(path).await {
            let mut cache = self.cache.write().await;
            cache.insert(
                path.to_path_buf(),
                CachedFile {
                    content: content.clone(),
                    mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    size: meta.len(),
                },
            );
        }

        debug!("Read {} bytes from {:?}", content.len(), path);
        Ok(content)
    }

    /// Invalidate the cache entry for `path` (called after every write).
    async fn invalidate_cache(&self, path: &Path) {
        self.cache.write().await.remove(path);
    }

    /// Read memory content (cache-backed).
    pub async fn read(&self, mem_type: MemoryType) -> crate::Result<String> {
        let path = self.memory_path(mem_type);
        if !path.exists() {
            debug!("Memory file {:?} does not exist, returning empty", mem_type);
            return Ok(String::new());
        }
        self.read_with_cache(&path).await
    }

    /// Write memory content, applying head/tail truncation if over the per-file cap.
    pub async fn write(&self, mem_type: MemoryType, content: &str) -> crate::Result<()> {
        // Apply head/tail truncation (preserves beginning + end of large files).
        let content_owned;
        let content = if content.len() > self.max_size {
            warn!(
                "Memory content exceeds max size ({} > {}), applying head/tail truncation",
                content.len(),
                self.max_size
            );
            content_owned = truncate_with_head_tail(content, self.max_size);
            &content_owned
        } else {
            content
        };

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

    /// Write without security checks (internal use only).
    async fn write_unchecked(&self, mem_type: MemoryType, content: &str) -> crate::Result<()> {
        let path = self.memory_path(mem_type);

        fs::write(&path, content)
            .await
            .map_err(|e| MantaError::Storage {
                context: format!("Failed to write memory file: {:?}", path),
                details: e.to_string(),
            })?;

        // Invalidate any cached version so the next read sees the new content.
        self.invalidate_cache(&path).await;
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

    /// Load `memory/*.md` fragments from the memory directory, sorted
    /// chronologically by filename (YYYY-MM-DD.md files sort naturally).
    pub async fn load_memory_fragments(&self) -> Vec<(String, String)> {
        let memory_dir = self.base_dir.join("memory");
        if !memory_dir.exists() {
            return vec![];
        }

        let mut entries = match fs::read_dir(&memory_dir).await {
            Ok(e) => e,
            Err(_) => return vec![],
        };

        let mut fragments: Vec<(String, String)> = vec![];
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let content = self.read_with_cache(&path).await.unwrap_or_default();
                if !content.is_empty() {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("memory")
                        .to_string();
                    // Apply per-file cap to each fragment.
                    let content = truncate_with_head_tail(&content, self.max_size);
                    fragments.push((name, content));
                }
            }
        }

        // Sort chronologically (dated files like YYYY-MM-DD.md sort naturally).
        fragments.sort_by(|a, b| a.0.cmp(&b.0));
        fragments
    }

    /// Get memory content formatted for system prompt.
    ///
    /// Applies the per-file cap via head/tail truncation and enforces the
    /// total character budget across all sections.
    pub async fn format_for_prompt(&self) -> crate::Result<String> {
        // OpenClaw-style personality files (loaded in priority order)
        // AGENTS.md and TOOLS.md are loaded first as they provide operating instructions
        let agents = self.read(MemoryType::Agents).await?;
        let tools_mem = self.read(MemoryType::Tools).await?;
        let identity = self.read(MemoryType::Identity).await?;
        let soul = self.read(MemoryType::Soul).await?;
        let bootstrap = self.read(MemoryType::Bootstrap).await?;
        let user = self.read(MemoryType::User).await?;
        let fragments = self.load_memory_fragments().await;

        let mut sections = Vec::new();
        let mut total_chars: usize = 0;

        /// Push a section if it is non-empty and fits in the total budget.
        macro_rules! push_section {
            ($content:expr, $label:expr) => {{
                let c = truncate_with_head_tail($content.trim(), self.max_size);
                if !c.is_empty() {
                    let section = format!("## {}\n{}\n", $label, c);
                    total_chars += section.len();
                    if total_chars <= self.total_max_size {
                        sections.push(section);
                    } else {
                        debug!(
                            "Total memory budget ({} chars) exceeded; skipping '{}'",
                            self.total_max_size, $label
                        );
                    }
                }
            }};
        }

        // AGENTS.md - Operating instructions (highest priority after system)
        push_section!(&agents, "Agents");
        // TOOLS.md - Tool conventions and notes
        push_section!(&tools_mem, "Tools");
        push_section!(&identity, "Identity");
        push_section!(&soul, "Soul");
        push_section!(&bootstrap, "Bootstrap");
        push_section!(&user, "User");

        // Memory fragments from memory/*.md
        if !fragments.is_empty() {
            let mut frag_parts = Vec::new();
            for (name, content) in &fragments {
                let c = truncate_with_head_tail(content.trim(), self.max_size);
                if !c.is_empty() {
                    let part = format!("### {}\n{}\n", name, c);
                    total_chars += part.len();
                    if total_chars <= self.total_max_size {
                        frag_parts.push(part);
                    }
                }
            }
            if !frag_parts.is_empty() {
                sections.push(format!("## Memory Fragments\n{}", frag_parts.join("\n")));
            }
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
            ("command_injection", r"(?i)(;|\|\||&&|`|<\(|>\$)\s*[a-z]+"),
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

        // Create memory/ subdirectory for dated/named fragments.
        let memory_dir = self.base_dir.join("memory");
        if !memory_dir.exists() {
            if let Err(e) = fs::create_dir_all(&memory_dir).await {
                warn!("Failed to create memory fragment directory {:?}: {}", memory_dir, e);
            }
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
                    Ok(ToolExecutionResult::success(format!("Memory content:\n{}", content))
                        .with_data(json!({
                            "memory_type": mem_type_str,
                            "content": content,
                            "size": content.len()
                        })))
                }

                "write" => {
                    let content = args["content"].as_str().ok_or_else(|| {
                        MantaError::Validation("content is required for write action".to_string())
                    })?;

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
                    let content = args["content"].as_str().ok_or_else(|| {
                        MantaError::Validation("content is required for append action".to_string())
                    })?;

                    self.memory.append(mem_type, content).await?;
                    Ok(ToolExecutionResult::success(format!("Appended to {:?}", mem_type))
                        .with_data(json!({"memory_type": mem_type_str})))
                }

                "clear" => {
                    self.memory.clear(mem_type).await?;
                    Ok(ToolExecutionResult::success(format!("Cleared {:?}", mem_type))
                        .with_data(json!({"memory_type": mem_type_str})))
                }

                _ => Err(MantaError::Validation(format!("Unknown action: {}", action))),
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
        let memory = PersonalityMemory::with_dir(temp_dir.clone()).await.unwrap();

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
    async fn test_personality_memory_size_limit_head_tail() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        // Use max_size=20 so the 100-char string triggers head/tail truncation.
        let memory = PersonalityMemory::with_dir(temp_dir.clone())
            .await
            .unwrap()
            .with_max_size(20);

        let long_content: String = "A".repeat(100);
        memory.write(MemoryType::Soul, &long_content).await.unwrap();

        let content = memory.read(MemoryType::Soul).await.unwrap();
        // Head/tail truncation produces head(14) + marker + tail(4) which is >20
        // but <100, and the truncation marker must be present.
        assert!(content.contains("[... ") && content.contains("chars truncated ...]"));
        assert!(content.len() < long_content.len());
    }

    #[tokio::test]
    async fn test_truncate_with_head_tail() {
        let content = "A".repeat(100);
        let result = truncate_with_head_tail(&content, 50);
        assert!(result.contains("[... ") && result.contains("chars truncated ...]"));
        // Result is shorter than original
        assert!(result.len() < content.len());
    }

    #[tokio::test]
    async fn test_truncate_with_head_tail_no_op() {
        let content = "Short content";
        let result = truncate_with_head_tail(content, 100);
        assert_eq!(result, content);
    }

    #[tokio::test]
    async fn test_personality_memory_exists() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = PersonalityMemory::with_dir(temp_dir.clone()).await.unwrap();

        assert!(!memory.exists(MemoryType::Bootstrap).await);

        memory
            .write(MemoryType::Bootstrap, "content")
            .await
            .unwrap();

        assert!(memory.exists(MemoryType::Bootstrap).await);
    }

    #[tokio::test]
    async fn test_memory_fragments_loaded_and_sorted() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = PersonalityMemory::with_dir(temp_dir.clone()).await.unwrap();

        // Create the memory/ subdir and two dated fragments.
        let mem_dir = temp_dir.join("memory");
        tokio::fs::create_dir_all(&mem_dir).await.unwrap();
        tokio::fs::write(mem_dir.join("2026-03-20.md"), "March content")
            .await
            .unwrap();
        tokio::fs::write(mem_dir.join("2026-01-01.md"), "January content")
            .await
            .unwrap();

        let frags = memory.load_memory_fragments().await;
        assert_eq!(frags.len(), 2);
        // Sorted chronologically, January comes first.
        assert_eq!(frags[0].0, "2026-01-01.md");
        assert_eq!(frags[1].0, "2026-03-20.md");
    }

    #[tokio::test]
    async fn test_memory_fragments_appear_in_prompt() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = PersonalityMemory::with_dir(temp_dir.clone()).await.unwrap();

        let mem_dir = temp_dir.join("memory");
        tokio::fs::create_dir_all(&mem_dir).await.unwrap();
        tokio::fs::write(mem_dir.join("notes.md"), "Important note")
            .await
            .unwrap();

        let prompt = memory.format_for_prompt().await.unwrap();
        assert!(prompt.contains("Memory Fragments"));
        assert!(prompt.contains("notes.md"));
        assert!(prompt.contains("Important note"));
    }

    #[tokio::test]
    async fn test_file_cache_returns_same_content() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = PersonalityMemory::with_dir(temp_dir.clone()).await.unwrap();

        memory.write(MemoryType::Identity, "v1").await.unwrap();

        // First read populates cache.
        let r1 = memory.read(MemoryType::Identity).await.unwrap();
        // Second read should hit cache and return the same value.
        let r2 = memory.read(MemoryType::Identity).await.unwrap();
        assert_eq!(r1, r2);
        assert_eq!(r1, "v1");
    }

    #[tokio::test]
    async fn test_file_cache_invalidated_on_write() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let memory = PersonalityMemory::with_dir(temp_dir.clone()).await.unwrap();

        memory.write(MemoryType::Identity, "v1").await.unwrap();
        let _ = memory.read(MemoryType::Identity).await.unwrap(); // populate cache

        memory.write(MemoryType::Identity, "v2").await.unwrap();
        let content = memory.read(MemoryType::Identity).await.unwrap();
        assert_eq!(content, "v2");
    }

    #[tokio::test]
    async fn test_total_budget_enforced() {
        let temp_dir = std::env::temp_dir().join(format!("manta_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        // Very small total budget so only the first section (Agents) fits.
        // Budget: "## Agents\n" (10) + content (20) + "\n" (1) = 31 chars fits.
        // "## Soul\n" (8) + soul_content (20) + "\n" (1) = 29 chars would push total to 60, exceeding 58.
        let memory = PersonalityMemory::with_dir(temp_dir.clone())
            .await
            .unwrap()
            .with_total_max_size(58);

        memory
            .write(MemoryType::Agents, "AgentContent1234567890")
            .await
            .unwrap();
        memory
            .write(MemoryType::Soul, "SoulShouldBeExcluded")
            .await
            .unwrap();

        let prompt = memory.format_for_prompt().await.unwrap();
        assert!(prompt.contains("AgentContent"));
        // Soul should be cut due to budget.
        assert!(!prompt.contains("SoulShouldBeExcluded"));
    }
}
