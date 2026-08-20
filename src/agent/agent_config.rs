//! [`AgentConfig`]: defaults, system-prompt composition, and workspace
//! directory resolution.

use tracing::warn;

use super::reflection;

/// Configuration for the Agent
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentConfig {
    /// The system prompt to use
    pub system_prompt: String,
    /// Maximum context window size (in tokens)
    pub max_context_tokens: usize,
    /// Maximum number of concurrent tool calls
    pub max_concurrent_tools: usize,
    /// Default temperature for completions
    pub temperature: f32,
    /// Maximum tokens per completion
    pub max_tokens: u32,
    /// Skills prompt (appended to system prompt)
    pub skills_prompt: Option<String>,
    /// Hard cap on conversation turns kept in context.
    ///
    /// When set, the oldest user+assistant pairs are dropped once this limit is
    /// exceeded.  `None` disables turn-based limiting (default).
    pub max_turns: Option<usize>,
    /// Workspace directory for file operations.
    /// When set, all relative paths are resolved against this directory.
    /// When `workspace_only` is true, file operations are restricted to this
    /// directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<std::path::PathBuf>,
    /// When true, restrict file operations to `workspace_dir`.
    #[serde(default = "default_true")]
    pub workspace_only: bool,
    /// Model to use for LLM-powered context compaction.
    ///
    /// When `None`, the agent's primary model is used.  Set to a cheaper/faster
    /// model (e.g. `"claude-haiku-4-5-20251101"`) to reduce compaction costs.
    pub compaction_model: Option<String>,
    /// Per-agent heartbeat configuration override.
    ///
    /// When `None`, inherits from global `GatewayConfig.heartbeat`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<crate::heartbeat::HeartbeatConfig>,
    /// Agent identifier — used to derive the default workspace directory.
    ///
    /// Set automatically when the agent is spawned. Not persisted in config
    /// files because it is implied by the file path / agent name.
    #[serde(skip)]
    pub agent_id: Option<String>,
    /// Optional reflection configuration.
    ///
    /// When set, agent responses are self-critiqued and iteratively improved
    /// by an LLM critic before being returned to the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflection_config: Option<reflection::ReflectionConfig>,
}

/// Serde default: `true`.
fn default_true() -> bool {
    true
}

impl Default for AgentConfig {
    fn default() -> Self {
        let system_prompt = r#"# Syscity AI Assistant

You are Syscity, a helpful AI assistant running locally on the user's machine.

## Core Rules (Priority: Highest)

1. Use ONLY tools explicitly provided in the tools list for this conversation — never invent or hallucinate tool names.
2. If a tool call fails, try a different approach or acknowledge the failure — do NOT repeat the same failed call.
3. NEVER modify Syscity's core configuration files (config.toml, GatewayConfig, or system-level ~/.syscity/ config). You MAY edit your own agent personality files (SOUL.md, IDENTITY.md, HEARTBEAT.md, MEMORY.md, etc.) in your agent directory when explicitly asked.
4. When editing IDENTITY.md, preserve the `## name` section followed by the display name on the next line.
5. When editing SOUL.md, preserve the YAML frontmatter between `---` lines and keep the `emoji:` field.

## Response Format

Use rich formatting for lists, structured data, and technical content:
- **Lists/Rankings**: markdown headings with bold metrics per item
- **Summaries**: tables with key takeaway
- **Technical Content**: inline code for commands/variables, code blocks with language tags, emoji indicators where appropriate (bug, performance, security)"#.to_string();

        Self {
            system_prompt,
            max_context_tokens: 16384,
            max_concurrent_tools: 5,
            temperature: 0.7,
            max_tokens: 2048,
            skills_prompt: None,
            max_turns: None,
            compaction_model: None,
            workspace_dir: None,
            workspace_only: false,
            heartbeat: None,
            agent_id: None,
            reflection_config: None,
        }
    }
}

impl AgentConfig {
    /// Get the full system prompt including skills
    pub fn full_system_prompt(&self) -> String {
        match &self.skills_prompt {
            Some(skills) => format!("{}\n\n## Skills\n\n{}", self.system_prompt, skills),
            None => self.system_prompt.clone(),
        }
    }

    /// Resolve the effective workspace directory for this agent.
    ///
    /// Resolution order:
    /// 1. `workspace_dir` config value (with `~` expanded)
    /// 2. For the default agent: `~/.syscity/workspace`
    /// 3. For named agents: `~/.syscity/agents/{agent_id}/workspace`
    pub fn resolve_workspace_dir(&self) -> std::path::PathBuf {
        match &self.workspace_dir {
            Some(dir) => crate::dirs::resolve_tilde(dir),
            None => match self.agent_id.as_deref() {
                Some("default") | None => crate::dirs::workspace_data_dir(),
                Some(id) => crate::dirs::agent_workspace_dir(id),
            },
        }
    }

    /// Get the full system prompt including personality memory and skills.
    ///
    /// Reads SOUL.md with structured YAML frontmatter support. If frontmatter
    /// is present, a structured "Agent Profile" section is injected before
    /// the free-form body.
    pub async fn full_system_prompt_with_personality(&self) -> String {
        let base_prompt = self.full_system_prompt();

        // Load personality memory
        let result = match crate::memory::PersonalityMemory::new().await {
            Ok(memory) => {
                // Initialize default files if they don't exist
                match memory.initialize_defaults().await {
                    Ok(_) => {}
                    Err(e) => warn!("Failed to initialize personality memory defaults: {}", e),
                }

                // Try structured SOUL.md first
                let soul_enhanced = match memory.read_soul().await {
                    Ok(soul_file) if soul_file.has_frontmatter => {
                        // Structured frontmatter: inject profile fragment + body
                        let profile = soul_file.config.to_prompt_fragment();
                        let body = soul_file.body;
                        if !body.is_empty() {
                            if profile.is_empty() {
                                format!("\n### Soul\n{}\n", body)
                            } else {
                                format!("{}\n### Soul\n{}\n", profile, body)
                            }
                        } else {
                            profile
                        }
                    }
                    _ => {
                        // Fallback: raw text without frontmatter
                        memory
                            .read(crate::memory::MemoryType::Soul)
                            .await
                            .unwrap_or_default()
                    }
                };

                let other_personality = memory
                    .format_for_prompt_with_context(crate::memory::MemoryContext::Primary)
                    .await
                    .unwrap_or_default();

                let mut parts = vec![base_prompt];
                if !soul_enhanced.is_empty() {
                    parts.push(soul_enhanced);
                }
                if !other_personality.is_empty() {
                    parts.push(other_personality);
                }
                parts.join("\n")
            }
            Err(_) => base_prompt,
        };

        // Inject host environment awareness so the LLM knows what OS
        // controls are available on this machine. The current time is NOT
        // baked into the system prompt (it would invalidate the KV-cache
        // prefix on every fresh thread and go stale in long-lived ones);
        // `Context::to_messages` appends it as a per-request user snapshot
        // instead.
        let host_env = crate::computer::platform::host_environment_summary();
        format!("{}\n\n## Host Environment\n\n{}", result, host_env)
    }
}
