//! Core Agent module for Manta
//!
//! The Agent is the central orchestrator that handles conversations,
//! manages context, calls tools, and interacts with LLM providers.

use crate::channels::{IncomingMessage, OutgoingMessage};
use crate::providers::{CompletionRequest, Message, Provider, Role, ToolCall, ToolResult};
use crate::tools::{ToolContext, ToolRegistry};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_stream::StreamExt;
use tracing::{debug, error, info, instrument, warn};

/// Progress events during message processing
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    /// Started processing
    Started,
    /// Executing a tool
    ToolCalling { name: String, arguments: String },
    /// Tool execution completed
    ToolResult { name: String, result: String },
    /// LLM is generating reasoning/thinking content
    Generating { content: Option<String> },
    /// LLM is streaming text content delta
    ContentDelta { text: String },
    /// Completed with final response
    Completed { response: String },
    /// Error occurred
    Error { message: String },
}

/// Callback type for progress updates
pub type ProgressCallback = Arc<
    dyn Fn(ProgressEvent) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync,
>;

pub mod acp;
pub mod artifacts;
pub mod budget;
pub mod compaction;
pub mod compressor;
pub mod context;
pub mod cost_guard;
pub mod disk_budget;
pub mod group;
pub mod personality;
pub mod planner;
pub mod prompt_builder;
pub mod route_resolution;
pub mod session;
pub mod session_files;
pub mod session_store;
pub mod subagent_registry;
pub mod todo;
pub mod transcript;
pub mod turns;

pub use acp::{
    AcpCommand, AcpController, AcpSessionStatus, ExecutionController, ExecutionMode, RuntimeState,
};
pub use artifacts::{Artifact, ArtifactStore, ArtifactStoreStats, ArtifactType};
pub use budget::{BudgetConfig, BudgetExhaustionAction, IterationBudget};
pub use compaction::{
    compute_context_hash, should_run_memory_flush, MemoryFlushConfig, SessionCompactionState,
    DEFAULT_MEMORY_FLUSH_FORCE_TRANSCRIPT_BYTES, DEFAULT_MEMORY_FLUSH_PROMPT,
    DEFAULT_MEMORY_FLUSH_RESERVE_TOKENS_FLOOR, DEFAULT_MEMORY_FLUSH_SOFT_TOKENS,
    DEFAULT_MEMORY_FLUSH_SYSTEM_PROMPT,
};
pub use compressor::{CompressionStats, CompressionStrategy, ContextCompressor};
pub use context::Context;
pub use cost_guard::CostGuard;
pub use disk_budget::{
    BudgetCategory, DiskBudgetError, DiskBudgetManager, EvictionStrategy, GlobalBudgetStats,
    SessionBudget, SessionBudgetStats,
};
pub use group::{
    GroupManagerStats, GroupMember, GroupRole, GroupSession, GroupSessionError, GroupSessionManager,
};
pub use personality::{AgentPersonality, AgentRegistry, PersonalityContext, SharedAgentRegistry};
pub use planner::{ActivePlan, TaskPlan, TaskPlanner};
pub use planner::PersistedPlan;
pub use prompt_builder::{ConversationPhase, PromptBuilder, PromptContext, TaskType};
pub use route_resolution::{
    BindingCache, BindingMode, ConversationScope, ResolvedBinding, RouteResolution, RouteResolver,
    RouteRule,
};
pub use session::{
    AgentInstanceStatus, MultiAgentSession, SessionAgent, SessionManager, SessionMessage,
    SessionStatus, ThreadBinding,
};
pub use session_files::{SessionFileDir, SessionFileManager};
pub use subagent_registry::{SubagentMetrics, SubagentRegistry, SubagentRun, SubagentStatus};
pub use todo::{Task, TaskStatus, TodoStore};
pub use transcript::{
    render_transcript, Transcript, TranscriptFormat, TranscriptMessage, TranscriptStore,
    TranscriptStoreStats,
};
pub use turns::{Thread, ThreadManager, Turn, TurnState};

use self::session_store::SessionStore;

/// Fast check for obviously time-sensitive queries
fn is_obviously_time_sensitive(message: &str) -> bool {
    let lower = message.to_lowercase();

    // Only check for obvious time keywords that clearly indicate real-time needs
    let obvious_time_queries = [
        "what time is it",
        "current time",
        "what's the time",
        "现在几点",
        "当前时间",
        "现在时间",
    ];

    for query in &obvious_time_queries {
        if lower.contains(query) {
            return true;
        }
    }

    false
}

/// Check if a message should be cached using LLM classification
/// Returns true if the response can be safely cached
async fn should_use_cache_llm(
    provider: &Arc<dyn Provider>,
    message: &str,
    model: Option<String>,
) -> bool {
    // Skip LLM check for obviously time-sensitive queries (optimization)
    if is_obviously_time_sensitive(message) {
        return false;
    }

    // Skip LLM check for very short queries (likely conversational)
    if message.len() < 20 {
        return false;
    }

    let prompt = format!(
        r#"Analyze this user query and determine if the response can be safely cached.

A query SHOULD be cached if:
- It's asking for general information, facts, summaries, or research
- The answer won't change significantly in the next hour
- Examples: "explain quantum computing", "summarize news", "how does X work"

A query should NOT be cached if:
- It asks for current time, date, or real-time data
- It asks for stock prices, crypto prices, or financial data
- It asks for current weather or temperature
- It asks "what is happening now" or "latest updates"
- The answer changes frequently (every minute/second)

User query: "{}"

Reply with ONLY "CACHE" or "NOCACHE"."#,
        message.replace('\"', "\\\"")
    );

    let request = CompletionRequest {
        model,
        messages: vec![Message::user(&prompt)],
        temperature: Some(0.0), // Deterministic
        max_tokens: Some(10),
        stream: false,
        ..Default::default()
    };

    match provider.complete(request).await {
        Ok(response) => {
            let content = response.message.content.trim().to_uppercase();
            // Default to not caching if LLM is uncertain
            content == "CACHE"
        }
        Err(_) => {
            // If LLM call fails, default to not caching for safety
            false
        }
    }
}

/// Determine if tools used are cacheable (time-sensitive tools skip caching)
fn are_tools_cacheable(tool_names: &[String]) -> bool {
    // Non-cacheable tools that return time-sensitive or real-time data
    let non_cacheable = [
        "datetime",
        "time",
        "clock",
        "date",
        "weather_current",
        "weather_now",
        "stock_price",
        "crypto_price",
    ];

    for tool in tool_names {
        let tool_lower = tool.to_lowercase();
        for nc in &non_cacheable {
            if tool_lower.contains(nc) {
                return false;
            }
        }
    }

    true
}

/// Cached response entry
#[derive(Debug, Clone)]
pub struct CachedResponse {
    pub response: String,
    pub created_at: SystemTime,
    pub tools_used: Vec<String>,
}

/// Simple in-memory response cache with TTL
#[derive(Debug, Clone)]
pub struct ResponseCache {
    cache: Arc<RwLock<HashMap<u64, CachedResponse>>>,
    ttl: Duration,
}

impl ResponseCache {
    /// Create a new response cache with specified TTL
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Generate a cache key from user message and context
    fn generate_key(user_id: &str, conversation_id: &str, message: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        user_id.hash(&mut hasher);
        conversation_id.hash(&mut hasher);
        message.trim().hash(&mut hasher);
        hasher.finish()
    }

    /// Get cached response if not expired
    pub async fn get(
        &self,
        user_id: &str,
        conversation_id: &str,
        message: &str,
    ) -> Option<CachedResponse> {
        let key = Self::generate_key(user_id, conversation_id, message);
        let cache = self.cache.read().await;

        if let Some(entry) = cache.get(&key) {
            if let Ok(elapsed) = entry.created_at.elapsed() {
                if elapsed < self.ttl {
                    return Some(entry.clone());
                }
            }
        }
        None
    }

    /// Store a response in cache
    pub async fn set(
        &self,
        user_id: &str,
        conversation_id: &str,
        message: &str,
        response: String,
        tools_used: Vec<String>,
    ) {
        let key = Self::generate_key(user_id, conversation_id, message);
        let entry = CachedResponse {
            response,
            created_at: SystemTime::now(),
            tools_used,
        };

        let mut cache = self.cache.write().await;
        cache.insert(key, entry);

        // Clean up old entries if cache is too large (> 1000 entries)
        if cache.len() > 1000 {
            let keys_to_remove: Vec<u64> = cache
                .iter()
                .filter(|(_, v)| v.created_at.elapsed().unwrap_or(Duration::MAX) > self.ttl)
                .map(|(k, _)| *k)
                .collect();

            for k in keys_to_remove {
                cache.remove(&k);
            }
        }
    }

    /// Clear expired entries
    pub async fn cleanup(&self) {
        let mut cache = self.cache.write().await;
        cache.retain(|_, v| v.created_at.elapsed().unwrap_or(Duration::MAX) < self.ttl);
    }
}

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
    /// When `workspace_only` is true, file operations are restricted to this directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<std::path::PathBuf>,
    /// When true, restrict file operations to `workspace_dir`.
    #[serde(default)]
    pub workspace_only: bool,
    /// Model to use for LLM-powered context compaction.
    ///
    /// When `None`, the agent's primary model is used.  Set to a cheaper/faster
    /// model (e.g. `"claude-haiku-4-5-20251101"`) to reduce compaction costs.
    pub compaction_model: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        let system_prompt = r#"# Manta AI Assistant

You are Manta, a helpful AI assistant running locally on the user's machine.

## Response Formatting Guidelines

When presenting information, especially lists or structured data, use rich formatting:

### For Lists/Rankings (e.g., "top 10 news", "best tools"):
```markdown
## Title

### 1. Item Name
- **Metric**: Value | **Other**: Value
- **Source**: Name
- **Description**: Brief description

### 2. Next Item...
```

### For Summaries:
```markdown
| Category | Count | Notes |
|----------|-------|-------|
| Type A | 5 | Description |
| Type B | 3 | Description |

**Key Takeaway**: Main insight here
```

### For Technical Content:
- Use `inline code` for commands/variables
- Use code blocks with language tags
- Include emoji indicators where appropriate (bug, performance, security)

## Current Time
The current time is provided in the context. When asked about time-sensitive information (news, weather, schedules), use the current time as reference."#.to_string();

        Self {
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
    /// 2. `~/.manta/workspace` (default)
    pub fn resolve_workspace_dir(&self) -> std::path::PathBuf {
        match &self.workspace_dir {
            Some(dir) => crate::dirs::resolve_tilde(dir),
            None => crate::dirs::workspace_data_dir(),
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
        match crate::memory::PersonalityMemory::new().await {
            Ok(memory) => {
                // Initialize default files if they don't exist
                let _ = memory.initialize_defaults().await;

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
        }
    }
}

/// The main Agent struct
#[derive(Clone)]
pub struct Agent {
    /// Agent configuration
    config: AgentConfig,
    /// The LLM provider
    provider: Arc<dyn Provider>,
    /// Model name to use (overrides provider default)
    model: Option<String>,
    /// Tool registry
    tools: Arc<ToolRegistry>,
    /// Per-conversation Thread (replaces flat `contexts` map).
    /// Thread owns the Context AND the turn log — conversation continuity lives here.
    thread_map: Arc<Mutex<HashMap<String, Thread>>>,
    /// Session store for turn persistence (optional).
    session_store: Option<Arc<SessionStore>>,
    /// Session ID used as namespace for turn persistence.
    session_id: Option<String>,
    /// Shutdown signal
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    /// Memory manager for unified memory operations (retrieval, storage, compaction)
    memory_manager: Option<Arc<crate::memory::MemoryManager>>,
    /// Memory store for persistence (legacy, prefer memory_manager)
    memory_store: Option<Arc<dyn crate::memory::MemoryStore>>,
    /// Chat history store for conversation persistence (legacy, prefer memory_manager)
    chat_history: Option<Arc<dyn crate::memory::ChatHistoryStore>>,
    /// Session search for conversation history indexing
    session_search: Option<Arc<crate::memory::SessionSearch>>,
    /// Response cache for identical prompts
    response_cache: Arc<ResponseCache>,
    /// Task planner for automatic task decomposition
    task_planner: Arc<TaskPlanner>,
    /// Active plans per conversation
    active_plans: Arc<RwLock<std::collections::HashMap<String, ActivePlan>>>,
    /// Live cost guard — checked before every provider call.
    cost_guard: Option<Arc<CostGuard>>,
    /// Active skill trust level for the current invocation.
    /// 0 = Community, 1 = Trusted (default).
    /// Set by the gateway before RunSkill invocations; reset to Trusted afterward.
    active_skill_trust: std::sync::Arc<std::sync::atomic::AtomicU8>,
    /// Skill manager for deterministic skill prefiltering.
    /// When set, skills are dynamically filtered based on user message triggers
    /// before being included in the system prompt.
    skill_manager: Option<Arc<RwLock<crate::skills::SkillManager>>>,
    /// Optional execution controller for pause/resume/step/cancel.
    /// Set by the ACP before dispatching a command; cleared afterward.
    execution_controller: Arc<RwLock<Option<Arc<ExecutionController>>>>,
    /// Optional max tool iteration override.
    /// Set by the ACP before dispatching a command; cleared afterward.
    max_tool_iterations_override: Arc<RwLock<Option<usize>>>,
    /// Transcript store for session conversation records.
    transcript_store: Option<Arc<crate::agent::TranscriptStore>>,
    /// Artifact store for session-bound artifacts (code, docs, links).
    artifact_store: Option<Arc<crate::agent::ArtifactStore>>,
    /// Disk budget manager for per-session storage quota.
    disk_budget: Option<Arc<crate::agent::DiskBudgetManager>>,
    /// Session file manager for isolated per-session file operations.
    session_file_manager: Option<Arc<crate::agent::SessionFileManager>>,
    /// Provider-specific extra parameters (e.g. thinking config) injected into completion requests.
    extra_params: Arc<RwLock<Option<serde_json::Value>>>,
    /// Optional model router for advanced routing, key rotation, and fallback.
    model_router: Option<Arc<crate::model_router::ModelRouter>>,
    /// Model alias used when routing through the model router.
    model_alias: Option<String>,
    /// Temporary model override set per-request (e.g. from OpenAI-compatible API).
    /// Takes precedence over `model_alias` and `model`.
    model_override: Arc<RwLock<Option<String>>>,
    /// Directory for persisting active plans (JSON files).
    plans_dir: Option<std::path::PathBuf>,
}

impl Agent {
    /// Create a new Agent
    pub fn new(config: AgentConfig, provider: Arc<dyn Provider>, tools: Arc<ToolRegistry>) -> Self {
        let provider_clone = provider.clone();

        Self {
            config,
            provider,
            model: None,
            tools,
            thread_map: Arc::new(Mutex::new(HashMap::new())),
            session_store: None,
            session_id: None,
            shutdown_tx: Arc::new(RwLock::new(None)),
            memory_manager: None,
            memory_store: None,
            chat_history: None,
            session_search: None,
            response_cache: Arc::new(ResponseCache::new(Duration::from_secs(3600))), // 1 hour TTL
            task_planner: Arc::new(TaskPlanner::new(provider_clone)),
            active_plans: Arc::new(RwLock::new(std::collections::HashMap::new())),
            cost_guard: None,
            active_skill_trust: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(1)), // Trusted
            skill_manager: None,
            execution_controller: Arc::new(RwLock::new(None)),
            max_tool_iterations_override: Arc::new(RwLock::new(None)),
            transcript_store: None,
            artifact_store: None,
            disk_budget: None,
            session_file_manager: None,
            extra_params: Arc::new(RwLock::new(None)),
            model_router: None,
            model_alias: None,
            model_override: Arc::new(RwLock::new(None)),
            plans_dir: None,
        }
    }

    /// Set provider-specific extra parameters (e.g. thinking config) to inject
    /// into every completion request.
    pub async fn set_extra_params(&self, params: Option<serde_json::Value>) {
        let mut guard = self.extra_params.write().await;
        *guard = params;
    }

    /// Set a temporary model override for the next provider call.
    ///
    /// Cleared automatically after each `process_message` invocation
    /// so that subsequent requests revert to the default model.
    pub async fn set_model_override(&self, model: Option<String>) {
        let mut guard = self.model_override.write().await;
        *guard = model;
    }

    /// Patch a [`CompletionRequest`] with provider-specific reasoning parameters
    /// when the target model is a known reasoning / thinking model and no
    /// explicit reasoning config has already been supplied via `extra`.
    fn patch_request_for_reasoning(&self, request: &mut CompletionRequest) {
        let family = self.provider.stream_family();
        let model = request
            .model
            .as_deref()
            .or(self.model.as_deref())
            .unwrap_or_else(|| self.provider.default_model());

        // Skip if user already provided explicit reasoning config in extra
        let has_reasoning_config = request.extra.as_ref().is_some_and(|v| {
            v.get("reasoning_effort").is_some()
                || v.get("thinking").is_some()
                || v.get("thinkingConfig").is_some()
        });
        if has_reasoning_config {
            return;
        }

        match family {
            crate::providers::stream_wrappers::ProviderStreamFamily::OpenAi
            | crate::providers::stream_wrappers::ProviderStreamFamily::OpenAiReasoning => {
                // OpenAI o1 / o3 series use `reasoning_effort`
                if model.starts_with("o1") || model.starts_with("o3") {
                    request.extra = Some(
                        serde_json::json!({ "reasoning_effort": "medium", "service_tier": "auto" }),
                    );
                }
            }
            crate::providers::stream_wrappers::ProviderStreamFamily::Anthropic
            | crate::providers::stream_wrappers::ProviderStreamFamily::AnthropicThinking => {
                // Anthropic thinking models (claude-3-7-sonnet-thinking, etc.)
                if model.contains("thinking") || model.contains("-extended-thinking") {
                    request.extra = Some(serde_json::json!({
                        "thinking": { "type": "enabled", "budget_tokens": 16000 }
                    }));
                }
            }
            crate::providers::stream_wrappers::ProviderStreamFamily::GoogleThinking => {
                // Gemini thinking models
                if model.contains("thinking") || model.contains("-exp") {
                    request.extra =
                        Some(serde_json::json!({ "thinkingConfig": { "thinkingBudget": 16000 } }));
                }
            }
            _ => {}
        }
    }

    /// Set the skill trust level for the next process_message invocation.
    ///
    /// Call this before invoking `process_message` or `process_message_with_progress`
    /// to constrain which tools the agent may call. The gateway resets this to
    /// `Trusted` after the invocation completes.
    pub fn set_skill_trust(&self, trust: crate::tools::SkillTrust) {
        use std::sync::atomic::Ordering;
        self.active_skill_trust
            .store(trust as u8, Ordering::Relaxed);
    }

    /// Read the current active skill trust level from the atomic.
    fn current_skill_trust(&self) -> crate::tools::SkillTrust {
        use std::sync::atomic::Ordering;
        match self.active_skill_trust.load(Ordering::Relaxed) {
            0 => crate::tools::SkillTrust::Community,
            _ => crate::tools::SkillTrust::Trusted,
        }
    }

    /// Build a ToolContext pre-configured with workspace settings from agent config.
    fn build_tool_context(
        &self,
        user_id: impl Into<String>,
        conversation_id: impl Into<String>,
    ) -> ToolContext {
        ToolContext::new(user_id, conversation_id)
            .with_skill_trust(self.current_skill_trust())
            .with_workspace_root(self.config.resolve_workspace_dir())
            .with_workspace_only(self.config.workspace_only)
    }

    /// Attach a `SessionStore` for turn persistence.
    ///
    /// When set, every completed turn is persisted asynchronously and the
    /// conversation history can be restored across restarts via
    /// [`Agent::restore_threads`].
    pub fn with_session_store(
        mut self,
        store: Arc<SessionStore>,
        session_id: impl Into<String>,
    ) -> Self {
        self.session_store = Some(store);
        self.session_id = Some(session_id.into());
        self
    }

    /// Attach a `CostGuard` to this agent.  When set, every provider call
    /// first checks `cost_guard.is_exceeded()` and returns an error if the
    /// budget has been exhausted.
    pub fn with_cost_guard(mut self, guard: Arc<CostGuard>) -> Self {
        self.cost_guard = Some(guard);
        self
    }

    /// Set the model name to use for completions
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let model = model.into();
        self.model = Some(model.clone());
        // Update task planner with the model
        let provider = self.provider.clone();
        self.task_planner = Arc::new(TaskPlanner::new(provider).with_model(model));
        self
    }

    /// Set the memory store
    pub fn with_memory_store(mut self, store: Arc<dyn crate::memory::MemoryStore>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Set the chat history store
    pub fn with_chat_history(mut self, store: Arc<dyn crate::memory::ChatHistoryStore>) -> Self {
        self.chat_history = Some(store);
        self
    }

    /// Set the session search for conversation indexing
    pub fn with_session_search(mut self, search: Arc<crate::memory::SessionSearch>) -> Self {
        self.session_search = Some(search);
        self
    }

    /// Set the memory manager for unified memory operations.
    ///
    /// The memory manager provides retrieval, storage, and compaction
    /// capabilities. When set, it takes precedence over the legacy
    /// memory_store and chat_history fields.
    pub fn with_memory_manager(mut self, manager: Arc<crate::memory::MemoryManager>) -> Self {
        self.memory_manager = Some(manager);
        self
    }

    /// Set the skill manager for deterministic skill prefiltering.
    ///
    /// When set, the agent will dynamically filter skills based on trigger patterns
    /// (regex, keywords, commands) before including them in the system prompt.
    /// This reduces token usage and improves relevance by only including skills
    /// that match the user's message.
    pub fn with_skill_manager(mut self, manager: Arc<RwLock<crate::skills::SkillManager>>) -> Self {
        self.skill_manager = Some(manager);
        self
    }

    /// Set the directory for persisting active plans.
    pub fn with_plans_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.plans_dir = Some(dir);
        self
    }

    /// Persist all active plans to disk.
    pub async fn save_all_plans(&self) -> crate::Result<()> {
        let Some(ref dir) = self.plans_dir else {
            return Ok(());
        };
        let plans = self.active_plans.read().await;
        for (conv_id, active) in plans.iter() {
            let snapshot = PersistedPlan::from_active(active);
            let path = dir.join(format!("{}.json", conv_id));
            if let Err(e) = snapshot.persist_to(&path).await {
                warn!("Failed to persist plan for {}: {}", conv_id, e);
            }
        }
        debug!("Persisted {} plans to {:?}", plans.len(), dir);
        Ok(())
    }

    /// Load previously persisted plans from disk and restore them.
    pub async fn load_plans(&self) -> crate::Result<usize> {
        let Some(ref dir) = self.plans_dir else {
            return Ok(0);
        };
        let persisted = planner::load_all_plans(dir).await?;
        let mut plans = self.active_plans.write().await;
        let mut count = 0;
        for pp in persisted {
            let conv_id = pp.plan.id.clone();
            let active = pp.into_active(&self.task_planner);
            plans.insert(conv_id, active);
            count += 1;
        }
        if count > 0 {
            info!("Restored {} plans from {:?}", count, dir);
        }
        Ok(count)
    }

    /// Attach a `TranscriptStore` for conversation recording.
    ///
    /// When set, every user and assistant message is appended to a
    /// per-session transcript that can be exported in multiple formats.
    pub fn with_transcript_store(mut self, store: Arc<crate::agent::TranscriptStore>) -> Self {
        self.transcript_store = Some(store);
        self
    }

    /// Attach an `ArtifactStore` for session-bound artifacts.
    ///
    /// When set, code blocks and documents produced during tool execution
    /// are automatically captured as artifacts.
    pub fn with_artifact_store(mut self, store: Arc<crate::agent::ArtifactStore>) -> Self {
        self.artifact_store = Some(store);
        self
    }

    /// Attach a `DiskBudgetManager` for per-session storage quota.
    ///
    /// When set, file operations are checked against the session's
    /// disk budget before proceeding.
    pub fn with_disk_budget(mut self, budget: Arc<crate::agent::DiskBudgetManager>) -> Self {
        self.disk_budget = Some(budget);
        self
    }

    /// Attach a `SessionFileManager` for isolated per-session file ops.
    ///
    /// When set, each conversation gets its own scoped directory.
    pub fn with_session_file_manager(
        mut self,
        manager: Arc<crate::agent::SessionFileManager>,
    ) -> Self {
        self.session_file_manager = Some(manager);
        self
    }

    /// Attach a `ModelRouter` for advanced routing, key rotation, and fallback.
    pub fn with_model_router(mut self, router: Arc<crate::model_router::ModelRouter>) -> Self {
        self.model_router = Some(router);
        self
    }

    /// Set the model alias used when routing through the model router.
    pub fn with_model_alias(mut self, alias: impl Into<String>) -> Self {
        self.model_alias = Some(alias.into());
        self
    }

    /// Update agent configuration at runtime.
    ///
    /// Applies fields from `new_config` to the running agent.  The update is
    /// applied immediately; in-flight requests use the previous values.
    pub fn update_config(&mut self, new_config: AgentConfig) {
        self.config = new_config;
    }

    /// Get chat history for a conversation
    pub async fn get_chat_history(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> crate::Result<Vec<crate::memory::ChatMessage>> {
        if let Some(ref store) = self.chat_history {
            store.get_conversation_history(conversation_id, limit).await
        } else {
            Ok(Vec::new())
        }
    }

    /// Get the last conversation ID for a user
    pub async fn get_last_conversation(&self, user_id: &str) -> crate::Result<Option<String>> {
        if let Some(ref store) = self.chat_history {
            store.get_last_conversation(user_id).await
        } else {
            Ok(None)
        }
    }

    /// Build a fresh `Context` for a new conversation thread.
    ///
    /// This is called only when no existing [`Thread`] is found for a
    /// `conversation_id`.  It constructs the system prompt, applies token limits
    /// and dynamic tool iteration caps, but does NOT store anything — callers
    /// are responsible for wrapping the returned `Context` in a `Thread` and
    /// inserting it into `thread_map`.
    async fn build_fresh_context(
        &self,
        conversation_id: &str,
        user_id: &str,
        user_message: &str,
    ) -> Context {
        // Build dynamic prompt context
        let mut prompt_ctx = PromptContext::new(user_message);
        prompt_ctx.detect_task_type();
        // New thread → no prior history; phase starts at Initial.
        prompt_ctx = prompt_ctx.set_phase(0);

        // Check for active plan
        let active_plans = self.active_plans.read().await;
        if let Some(active_plan) = active_plans.get(conversation_id) {
            if let Some(task_prompt) = active_plan.current_task_prompt() {
                prompt_ctx.task_context = Some(task_prompt);
            }
        }
        drop(active_plans);

        // Get available tools
        let tool_context = self.build_tool_context(user_id, conversation_id);
        let tool_defs = self.tools.get_available(&tool_context);
        prompt_ctx.available_tools = tool_defs;

        // Get base prompt
        let base_prompt = self.config.full_system_prompt_with_personality().await;

        // Retrieve relevant memories via MemoryManager and inject into context
        let memory_context = if let Some(ref mm) = self.memory_manager {
            match mm
                .session_context(user_id, conversation_id, Some(user_message))
                .await
            {
                Ok(ctx) => {
                    let formatted = ctx.format_for_injection();
                    if formatted.is_empty() {
                        None
                    } else {
                        Some(formatted)
                    }
                }
                Err(e) => {
                    tracing::warn!("Memory context retrieval failed: {}", e);
                    None
                }
            }
        } else {
            None
        };

        // Combine base prompt with memory context and skills
        let full_prompt = {
            let mut prompt = base_prompt;

            // Add memory context if available
            if let Some(ref mem_ctx) = memory_context {
                prompt = format!("{}\n\n{}", prompt, mem_ctx);
            }

            // Add dynamically filtered skills based on user message
            if let Some(ref skill_manager) = self.skill_manager {
                debug!("SkillManager is active, prefiltering skills");
                let mgr = skill_manager.read().await;
                let matching_skills = mgr.prefilter_skills(user_message, 5).await;
                if !matching_skills.is_empty() {
                    let skills_text = matching_skills
                        .iter()
                        .map(|s| s.to_prompt_section())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    prompt = format!("{}\n\n## Active Skills\n\n{}", prompt, skills_text);
                }
            } else if let Some(ref static_skills) = self.config.skills_prompt {
                // Fallback to static skills prompt if skill_manager not set
                prompt = format!("{}\n\n{}", prompt, static_skills);
            }

            prompt
        };

        // Build dynamic system prompt
        let system_prompt = PromptBuilder::build_from_context(
            &full_prompt,
            &prompt_ctx,
            self.config.max_context_tokens / 4, // Rough token estimate
        );

        let mut context = Context::new(
            conversation_id.to_string(),
            system_prompt,
            self.config.max_context_tokens,
        );

        // Apply turn cap from config so the agent never accumulates an
        // unbounded conversation history.
        if let Some(max_turns) = self.config.max_turns {
            context = context.with_max_turns(max_turns);
        }

        // Set dynamic tool iteration limit based on message complexity
        let dynamic_limit = Context::calculate_dynamic_limit(user_message);
        context.set_max_tool_iterations(dynamic_limit);
        info!(
            "Set dynamic tool iteration limit: {} for conversation {}",
            dynamic_limit, conversation_id
        );

        // Apply ACP max iteration override if set
        let override_opt = *self.max_tool_iterations_override.read().await;
        if let Some(max_iter) = override_opt {
            context.set_max_tool_iterations(max_iter);
            info!(
                "Applied ACP max iteration override: {} for conversation {}",
                max_iter, conversation_id
            );
        }

        context
    }

    /// Process an incoming message
    #[instrument(skip(self, message))]
    pub async fn process_message(
        &self,
        message: IncomingMessage,
    ) -> crate::Result<OutgoingMessage> {
        debug!("Processing message from user: {}", message.user_id);

        let conversation_id = message.conversation_id.0.clone();
        let user_id = message.user_id.0.clone();
        let content = message.content.clone();

        // Check cache for identical prompt (only for non-follow-up, non-time-sensitive messages)
        // Skip cache if this looks like a follow-up (short message referring to previous context)
        let is_follow_up = content.len() < 50
            && (content.contains("it")
                || content.contains("that")
                || content.contains("this")
                || content.contains("上面的")
                || content.contains("这个")
                || content.contains("那个"));

        // Use LLM to determine if query should be cached
        let should_cache = !is_follow_up
            && should_use_cache_llm(&self.provider, &content, self.model.clone()).await;

        if should_cache {
            if let Some(cached) = self
                .response_cache
                .get(&user_id, &conversation_id, &content)
                .await
            {
                info!("Cache hit for user {} - returning cached response", user_id);

                // Store user message in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::ChatMessage;
                    let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store user message: {}", e);
                    }
                }

                // Store cached assistant response in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::ChatMessage;
                    let chat_msg =
                        ChatMessage::new(&conversation_id, &user_id, "assistant", &cached.response);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store assistant message: {}", e);
                    }
                }

                // Return cached response
                return Ok(OutgoingMessage::new(
                    crate::channels::ConversationId(conversation_id),
                    cached.response.clone(),
                ));
            }
        }

        // Store user message in chat history and index for search
        let message_id = uuid::Uuid::new_v4().to_string();

        // Persist user message via MemoryManager (episodic memory)
        if let Some(ref mm) = self.memory_manager {
            if let Err(e) = mm
                .remember_message(&user_id, &conversation_id, "user", &content)
                .await
            {
                warn!("MemoryManager: failed to store user message: {}", e);
            }
        }

        if let Some(ref store) = self.chat_history {
            use crate::memory::ChatMessage;
            let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
            // Clone message_id before moving chat_msg
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store user message: {}", e);
            }
            // Index for session search
            if let Some(ref search) = self.session_search {
                if let Err(e) = search
                    .index_message(&msg_id, &conversation_id, &user_id, &content, "user")
                    .await
                {
                    error!("Failed to index user message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            // Even if chat history is not enabled, index for search
            if let Err(e) = search
                .index_message(&message_id, &conversation_id, &user_id, &content, "user")
                .await
            {
                error!("Failed to index user message for search: {}", e);
            }
        }

        // Record user message in transcript
        if let Some(ref transcript_store) = self.transcript_store {
            transcript_store.append(
                &conversation_id,
                "agent",
                &user_id,
                &conversation_id,
                TranscriptMessage::new("user", &content),
            );
            // Track transcript size in disk budget
            if let Some(ref budget) = self.disk_budget {
                let transcript_size = content.len();
                let _ = budget.track_item(
                    &conversation_id,
                    format!("transcript-user-{}", message_id),
                    BudgetCategory::Transcript,
                    transcript_size,
                );
            }
        }

        // Check if we need task planning
        let needs_planning = self.task_planner.needs_planning(&content).await;

        if needs_planning {
            info!("Complex task detected, creating plan for: {}", conversation_id);

            // Create a plan
            match self.task_planner.create_plan(&content).await {
                Ok(plan) => {
                    let summary = plan.format_summary();
                    info!("Created plan with {} tasks", plan.tasks.len());

                    // Convert to todos
                    let todos = self.task_planner.plan_to_todos(&plan);

                    // Store active plan
                    let active_plan = ActivePlan {
                        plan,
                        todos,
                        completed_tasks: Vec::new(),
                    };

                    let mut plans = self.active_plans.write().await;
                    plans.insert(conversation_id.clone(), active_plan);
                    drop(plans);

                    // Persist the plan if plans_dir is configured
                    if let Some(ref dir) = self.plans_dir {
                        let plans = self.active_plans.read().await;
                        if let Some(active) = plans.get(&conversation_id) {
                            let snapshot = PersistedPlan::from_active(active);
                            let path = dir.join(format!("{}.json", conversation_id));
                            if let Err(e) = snapshot.persist_to(&path).await {
                                warn!("Failed to persist plan: {}", e);
                            }
                        }
                    }

                    // Return the plan to the user
                    return Ok(OutgoingMessage::new(
                        crate::channels::ConversationId(conversation_id),
                        format!("I'll break this down into steps:\n\n{}", summary),
                    ));
                }
                Err(e) => {
                    warn!("Failed to create plan: {}, proceeding without planning", e);
                }
            }
        }

        // ── Thread take-out ───────────────────────────────────────────────────
        // Acquire the Thread for this conversation (or create a new one).
        // The lock is dropped before the LLM call to avoid holding it during
        // a long-running async operation.
        let mut thread = {
            let mut map = self.thread_map.lock().await;
            match map.remove(&conversation_id) {
                Some(t) => t,
                None => {
                    // First message for this conversation — build initial Context.
                    let ctx = self
                        .build_fresh_context(&conversation_id, &user_id, &content)
                        .await;
                    let thread_id = format!("thread-{}", &conversation_id);
                    // Persist the new thread record (fire-and-forget).
                    if let (Some(store), Some(sid)) =
                        (self.session_store.clone(), self.session_id.clone())
                    {
                        let tid = thread_id.clone();
                        let label = conversation_id.clone();
                        tokio::spawn(async move {
                            let _ = store
                                .save_thread(
                                    &sid,
                                    &tid,
                                    &label,
                                    chrono::Utc::now().timestamp_millis(),
                                )
                                .await;
                        });
                    }
                    Thread::from_context(thread_id, &conversation_id, ctx)
                }
            }
        };

        // Apply ACP max iteration override for existing threads
        let override_opt = *self.max_tool_iterations_override.read().await;
        if let Some(max_iter) = override_opt {
            thread.context.set_max_tool_iterations(max_iter);
            info!(
                "Applied ACP max iteration override to existing thread: {} for conversation {}",
                max_iter, conversation_id
            );
        }

        // Reset tool tracking and add user message for this turn.
        thread.context.clear_tools_used();
        thread
            .context
            .add_message(Message::user_named(&user_id, &content));

        // Track this turn in the turn log.
        let turn_idx = thread.push_turn(&content);
        thread.turns[turn_idx].start();

        // Check if we're executing an active plan
        let active_plan_check = {
            let plans = self.active_plans.read().await;
            plans.get(&conversation_id).map(|p| {
                (p.plan.progress_percent(), p.plan.current_task().map(|t| t.description.clone()))
            })
        };

        if let Some((progress, Some(current_task))) = active_plan_check {
            info!("Executing plan: {}% - Task: {}", progress, current_task);
        }

        // Get response from LLM (lock NOT held during this await).
        let llm_result = self.get_completion(&mut thread.context).await;

        // Complete or interrupt the turn based on result.
        let llm_result = match llm_result {
            Ok(resp) => {
                let asst_text = resp.message.content.clone();
                thread.turns[turn_idx].complete(asst_text.clone());
                // Persist the turn asynchronously (fire-and-forget).
                if let (Some(store), Some(sid)) =
                    (self.session_store.clone(), self.session_id.clone())
                {
                    let tid = thread.id.clone();
                    let user_c = content.clone();
                    let t_idx = turn_idx as i64;
                    tokio::spawn(async move {
                        let _ = store
                            .append_turn(&sid, &tid, t_idx, &user_c, &asst_text, "complete")
                            .await;
                    });
                }
                Ok(resp)
            }
            Err(e) => {
                thread.turns[turn_idx].mark_error();
                Err(e)
            }
        };

        // Collect tools_used BEFORE putting thread back (needed for cache logic below).
        let tools_used_this_turn = thread.context.tools_used().to_vec();

        // ── Put thread back ───────────────────────────────────────────────────
        {
            let mut map = self.thread_map.lock().await;
            map.insert(conversation_id.clone(), thread);
        }

        let response = llm_result?;

        // Mark memory hits based on response content
        if let Some(ref mm) = self.memory_manager {
            let session_key = format!("{}:{}", user_id, conversation_id);
            mm.evaluate_response_hits(&session_key, &response.message.content)
                .await;
            // Close the effectiveness feedback loop
            mm.apply_effectiveness_adjustments().await;
        }

        // Store assistant response in chat history and index for search
        let assistant_message_id = uuid::Uuid::new_v4().to_string();

        // Record assistant message in transcript
        if let Some(ref transcript_store) = self.transcript_store {
            transcript_store.append(
                &conversation_id,
                "agent",
                &user_id,
                &conversation_id,
                TranscriptMessage::new("assistant", &response.message.content),
            );
            // Track transcript size in disk budget
            if let Some(ref budget) = self.disk_budget {
                let transcript_size = response.message.content.len();
                let _ = budget.track_item(
                    &conversation_id,
                    format!("transcript-assistant-{}", assistant_message_id),
                    BudgetCategory::Transcript,
                    transcript_size,
                );
            }
        }

        // Persist assistant response via MemoryManager (episodic memory)
        if let Some(ref mm) = self.memory_manager {
            if let Err(e) = mm
                .remember_message(
                    &user_id,
                    &conversation_id,
                    "assistant",
                    &response.message.content,
                )
                .await
            {
                warn!("MemoryManager: failed to store assistant message: {}", e);
            }
        }

        if let Some(ref store) = self.chat_history {
            use crate::memory::ChatMessage;
            let chat_msg = ChatMessage::new(
                &conversation_id,
                &user_id,
                "assistant",
                &response.message.content,
            );
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store assistant message: {}", e);
            }
            // Index for session search
            if let Some(ref search) = self.session_search {
                if let Err(e) = search
                    .index_message(
                        &msg_id,
                        &conversation_id,
                        &user_id,
                        &response.message.content,
                        "assistant",
                    )
                    .await
                {
                    error!("Failed to index assistant message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            // Even if chat history is not enabled, index for search
            if let Err(e) = search
                .index_message(
                    &assistant_message_id,
                    &conversation_id,
                    &user_id,
                    &response.message.content,
                    "assistant",
                )
                .await
            {
                error!("Failed to index assistant message for search: {}", e);
            }
        }

        // Only cache the response if it should be cached
        if should_cache {
            // Check if tools used are cacheable (skip cache for time-sensitive tools)
            if are_tools_cacheable(&tools_used_this_turn) {
                self.response_cache
                    .set(
                        &user_id,
                        &conversation_id,
                        &content,
                        response.message.content.clone(),
                        tools_used_this_turn,
                    )
                    .await;
            }
        }

        // Create outgoing message with usage tracking
        let mut outgoing = OutgoingMessage::new(
            crate::channels::ConversationId(conversation_id),
            response.message.content.clone(),
        );
        if let Some(ref usage) = response.usage {
            outgoing.usage = Some(*usage);
        }

        Ok(outgoing)
    }

    /// Process an incoming message with progress callbacks
    #[instrument(skip(self, message, progress_cb))]
    pub async fn process_message_with_progress(
        &self,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
    ) -> crate::Result<OutgoingMessage> {
        debug!("Processing message with progress from user: {}", message.user_id);

        let conversation_id = message.conversation_id.0.clone();
        let user_id = message.user_id.0.clone();
        let content = message.content.clone();

        // Notify started
        (progress_cb)(ProgressEvent::Started).await;

        // Check cache for identical prompt (only for non-follow-up, non-time-sensitive messages)
        let is_follow_up = content.len() < 50
            && (content.contains("it")
                || content.contains("that")
                || content.contains("this")
                || content.contains("上面的")
                || content.contains("这个")
                || content.contains("那个"));

        // Use LLM to determine if query should be cached
        let should_cache = !is_follow_up
            && should_use_cache_llm(&self.provider, &content, self.model.clone()).await;

        if should_cache {
            if let Some(cached) = self
                .response_cache
                .get(&user_id, &conversation_id, &content)
                .await
            {
                info!("Cache hit for user {} - returning cached response", user_id);

                // Notify cache hit
                (progress_cb)(ProgressEvent::ToolCalling {
                    name: "cache".to_string(),
                    arguments: "{\"hit\": true}".to_string(),
                })
                .await;

                // Store user message in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::ChatMessage;
                    let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store user message: {}", e);
                    }
                }

                // Store cached assistant response in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::ChatMessage;
                    let chat_msg =
                        ChatMessage::new(&conversation_id, &user_id, "assistant", &cached.response);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store assistant message: {}", e);
                    }
                }

                // Notify completed with cached response
                (progress_cb)(ProgressEvent::Completed {
                    response: cached.response.clone(),
                })
                .await;

                // Return cached response
                return Ok(OutgoingMessage::new(
                    crate::channels::ConversationId(conversation_id),
                    cached.response.clone(),
                ));
            }
        }

        // Store user message in chat history and index for search
        let message_id = uuid::Uuid::new_v4().to_string();
        if let Some(ref store) = self.chat_history {
            use crate::memory::ChatMessage;
            let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store user message: {}", e);
            }
            if let Some(ref search) = self.session_search {
                if let Err(e) = search
                    .index_message(&msg_id, &conversation_id, &user_id, &content, "user")
                    .await
                {
                    error!("Failed to index user message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            if let Err(e) = search
                .index_message(&message_id, &conversation_id, &user_id, &content, "user")
                .await
            {
                error!("Failed to index user message for search: {}", e);
            }
        }

        // Record user message in transcript
        if let Some(ref transcript_store) = self.transcript_store {
            transcript_store.append(
                &conversation_id,
                "agent",
                &user_id,
                &conversation_id,
                TranscriptMessage::new("user", &content),
            );
            if let Some(ref budget) = self.disk_budget {
                let _ = budget.track_item(
                    &conversation_id,
                    format!("transcript-user-{}", message_id),
                    BudgetCategory::Transcript,
                    content.len(),
                );
            }
        }

        // ── Thread take-out (same pattern as process_message) ────────────────
        let mut thread = {
            let mut map = self.thread_map.lock().await;
            match map.remove(&conversation_id) {
                Some(t) => t,
                None => {
                    let ctx = self
                        .build_fresh_context(&conversation_id, &user_id, &content)
                        .await;
                    let thread_id = format!("thread-{}", &conversation_id);
                    if let (Some(store), Some(sid)) =
                        (self.session_store.clone(), self.session_id.clone())
                    {
                        let tid = thread_id.clone();
                        let label = conversation_id.clone();
                        tokio::spawn(async move {
                            let _ = store
                                .save_thread(
                                    &sid,
                                    &tid,
                                    &label,
                                    chrono::Utc::now().timestamp_millis(),
                                )
                                .await;
                        });
                    }
                    Thread::from_context(thread_id, &conversation_id, ctx)
                }
            }
        };

        // Apply ACP max iteration override for existing threads
        let override_opt = *self.max_tool_iterations_override.read().await;
        if let Some(max_iter) = override_opt {
            thread.context.set_max_tool_iterations(max_iter);
            info!(
                "Applied ACP max iteration override to existing thread: {} for conversation {}",
                max_iter, conversation_id
            );
        }

        // Reset tool tracking and add user message for this turn.
        thread.context.clear_tools_used();
        thread
            .context
            .add_message(Message::user_named(&user_id, &content));

        // Track this turn.
        let turn_idx = thread.push_turn(&content);
        thread.turns[turn_idx].start();

        // Get response from LLM with progress (lock NOT held).
        let llm_result = self
            .get_completion_with_progress(&mut thread.context, progress_cb.clone())
            .await;

        // Complete or interrupt the turn.
        let llm_result = match llm_result {
            Ok(resp) => {
                let asst_text = resp.message.content.clone();
                thread.turns[turn_idx].complete(asst_text.clone());
                if let (Some(store), Some(sid)) =
                    (self.session_store.clone(), self.session_id.clone())
                {
                    let tid = thread.id.clone();
                    let user_c = content.clone();
                    let t_idx = turn_idx as i64;
                    tokio::spawn(async move {
                        let _ = store
                            .append_turn(&sid, &tid, t_idx, &user_c, &asst_text, "complete")
                            .await;
                    });
                }
                Ok(resp)
            }
            Err(e) => {
                thread.turns[turn_idx].mark_error();
                Err(e)
            }
        };

        let tools_used_this_turn = thread.context.tools_used().to_vec();

        // ── Put thread back ───────────────────────────────────────────────────
        {
            let mut map = self.thread_map.lock().await;
            map.insert(conversation_id.clone(), thread);
        }

        let response = llm_result?;

        // Store assistant response
        let assistant_message_id = uuid::Uuid::new_v4().to_string();

        // Record assistant message in transcript
        if let Some(ref transcript_store) = self.transcript_store {
            transcript_store.append(
                &conversation_id,
                "agent",
                &user_id,
                &conversation_id,
                TranscriptMessage::new("assistant", &response.message.content),
            );
            if let Some(ref budget) = self.disk_budget {
                let _ = budget.track_item(
                    &conversation_id,
                    format!("transcript-assistant-{}", assistant_message_id),
                    BudgetCategory::Transcript,
                    response.message.content.len(),
                );
            }
        }

        if let Some(ref store) = self.chat_history {
            use crate::memory::ChatMessage;
            let chat_msg = ChatMessage::new(
                &conversation_id,
                &user_id,
                "assistant",
                &response.message.content,
            );
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store assistant message: {}", e);
            }
            if let Some(ref search) = self.session_search {
                if let Err(e) = search
                    .index_message(
                        &msg_id,
                        &conversation_id,
                        &user_id,
                        &response.message.content,
                        "assistant",
                    )
                    .await
                {
                    error!("Failed to index assistant message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            if let Err(e) = search
                .index_message(
                    &assistant_message_id,
                    &conversation_id,
                    &user_id,
                    &response.message.content,
                    "assistant",
                )
                .await
            {
                error!("Failed to index assistant message for search: {}", e);
            }
        }

        // Only cache the response if it should be cached
        if should_cache
            && are_tools_cacheable(&tools_used_this_turn) {
                self.response_cache
                    .set(
                        &user_id,
                        &conversation_id,
                        &content,
                        response.message.content.clone(),
                        tools_used_this_turn,
                    )
                    .await;
            }

        // Notify completed
        let response_content = response.message.content.clone();
        (progress_cb)(ProgressEvent::Completed {
            response: response_content.clone(),
        })
        .await;

        // Create outgoing message with full metadata
        let mut outgoing = OutgoingMessage::new(
            crate::channels::ConversationId(conversation_id),
            response_content,
        );
        if let Some(ref reasoning) = response.message.reasoning_content {
            if !reasoning.is_empty() {
                outgoing.reasoning_content = Some(reasoning.clone());
            }
        }
        if let Some(ref calls) = response.message.tool_calls {
            if !calls.is_empty() {
                outgoing.tool_calls = Some(calls.clone());
            }
        }
        outgoing.usage = response.usage;

        Ok(outgoing)
    }

    /// Process a message in persistent session mode with an execution controller.
    ///
    /// The controller is attached before processing and detached afterward,
    /// enabling pause/resume/step/cancel during the tool-call loop.
    pub async fn process_message_with_controller(
        &self,
        message: IncomingMessage,
        controller: Arc<ExecutionController>,
        max_iterations: usize,
    ) -> crate::Result<OutgoingMessage> {
        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = Some(controller);
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = Some(max_iterations);
        }

        let result = self.process_message(message).await;

        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = None;
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = None;
        }

        result
    }

    /// Process a message with progress callbacks and an execution controller.
    pub async fn process_message_with_progress_and_controller(
        &self,
        message: IncomingMessage,
        progress_cb: ProgressCallback,
        controller: Arc<ExecutionController>,
        max_iterations: usize,
    ) -> crate::Result<OutgoingMessage> {
        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = Some(controller);
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = Some(max_iterations);
        }

        let result = self
            .process_message_with_progress(message, progress_cb)
            .await;

        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = None;
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = None;
        }

        result
    }

    /// Run a message in one-shot mode (no persistence) with an execution controller.
    ///
    /// The thread context is discarded after execution completes.
    pub async fn run_message_with_controller(
        &self,
        message: IncomingMessage,
        controller: Arc<ExecutionController>,
        max_iterations: usize,
    ) -> crate::Result<OutgoingMessage> {
        let conversation_id = message.conversation_id.0.clone();

        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = Some(controller);
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = Some(max_iterations);
        }

        let result = self.process_message(message).await;

        {
            let mut ctrl = self.execution_controller.write().await;
            *ctrl = None;
            let mut max_iter = self.max_tool_iterations_override.write().await;
            *max_iter = None;
        }

        // Run mode: discard the thread after execution
        {
            let mut map = self.thread_map.lock().await;
            map.remove(&conversation_id);
        }

        result
    }

    /// Get a completion from the LLM, handling tool calls
    async fn get_completion(
        &self,
        context: &mut Context,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        // If the context is over-budget, try to reduce it before sending.
        if context.needs_pruning() {
            if let Some(ref compaction_model) = self.config.compaction_model {
                // LLM-assisted compaction: produce a high-quality summary.
                let compressor = crate::agent::compressor::ContextCompressor::new(
                    self.config.max_context_tokens,
                );
                let history = context.history().to_vec();
                let compacted = compressor
                    .compact_with_llm(
                        &history,
                        &self.provider,
                        Some(compaction_model.as_str()),
                        2,
                        6,
                    )
                    .await;
                context.replace_messages(compacted);
            } else {
                // Fallback: drop middle messages and insert a placeholder summary.
                // This keeps the context coherent without an extra LLM call.
                context.summarize();
            }
        }

        let messages = context.to_messages();

        // Get available tools
        let tool_context = self.build_tool_context("user", context.id());
        let tool_defs = self.tools.get_available(&tool_context);
        let has_tools = !tool_defs.is_empty();

        let extra = self.extra_params.read().await.clone();
        let mut request = CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            stream: false,
            extra,
            ..Default::default()
        };
        self.patch_request_for_reasoning(&mut request);

        if has_tools && self.provider.supports_tools() {
            // Convert FunctionDefinition to ToolDefinition
            let tools: Vec<crate::providers::ToolDefinition> = tool_defs
                .into_iter()
                .map(|f| crate::providers::ToolDefinition {
                    tool_type: "function".to_string(),
                    function: f,
                })
                .collect();
            request.tools = Some(tools);
        }

        // Check live cost guard before calling provider
        if let Some(ref guard) = self.cost_guard {
            if guard.is_exceeded() {
                return Err(crate::error::MantaError::Validation(
                    "Budget limit exceeded — refusing provider call. \
                     Adjust daily_limit_cents or hourly_action_limit in config."
                        .to_string(),
                ));
            }
        }

        // Get completion — use model router when available for key rotation / fallback
        let response = if let Some(ref router) = self.model_router {
            let alias = {
                let guard = self.model_override.read().await;
                guard
                    .as_ref()
                    .cloned()
                    .or(self.model_alias.clone())
                    .or(self.model.clone())
                    .unwrap_or_else(|| self.provider.default_model().to_string())
            };
            router.complete(&alias, request.messages).await?
        } else {
            self.provider.complete(request).await?
        };

        // Record token usage in cost guard
        if let Some(ref guard) = self.cost_guard {
            if let Some(ref usage) = response.usage {
                guard.record_usage(
                    usage.prompt_tokens as u64,
                    usage.completion_tokens as u64,
                    response.model.as_str(),
                );
            }
        }

        // Handle tool calls if present
        if let Some(tool_calls) = &response.message.tool_calls {
            if !tool_calls.is_empty() {
                debug!("Processing {} tool calls", tool_calls.len());
                return self.handle_tool_calls(context, &response, tool_calls).await;
            }
        }

        // Add assistant message to context
        context.add_message(response.message.clone());

        Ok(response)
    }

    /// Handle tool calls from the LLM
    async fn handle_tool_calls(
        &self,
        context: &mut Context,
        original_response: &crate::providers::CompletionResponse,
        tool_calls: &[ToolCall],
    ) -> crate::Result<crate::providers::CompletionResponse> {
        // Check iteration limit before processing
        if !context.increment_tool_iteration() {
            warn!("Tool iteration limit reached ({}), stopping", context.tool_iterations());

            // Return a response indicating the limit was reached
            return Ok(crate::providers::CompletionResponse {
                message: Message {
                    role: Role::Assistant,
                    content: format!("I've reached the maximum number of tool calls ({}) for this request. The task may be too complex or the tools may not be providing the expected results. Please try a more specific request or break the task into smaller steps.", Context::DEFAULT_MAX_TOOL_ITERATIONS),
                    reasoning_content: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    metadata: None,
                },
                usage: None,
                model: "system".to_string(),
                finish_reason: Some("tool_limit".to_string()),
            });
        }

        // Add assistant message with tool calls
        context.add_message(original_response.message.clone());

        // Execute tools concurrently (up to limit)
        let tool_context = self
            .build_tool_context("user", context.id())
            .with_timeout(std::time::Duration::from_secs(30));

        let mut results = Vec::new();

        for tool_call in tool_calls.iter().take(self.config.max_concurrent_tools) {
            let tool_name = tool_call.function.name.clone();
            let tool_args = tool_call.function.arguments.clone();

            // Check for duplicate tool calls
            if context.is_tool_call_duplicate(&tool_name, &tool_args) {
                warn!("Duplicate tool call detected: {} with same args, skipping", tool_name);
                results.push(ToolResult::error(
                    &tool_call.id,
                    "Error: This exact tool call was already executed. The previous result did not provide the expected data. Please try a different approach or acknowledge that the tool cannot fulfill this request."
                ));
                continue;
            }

            // Record this tool call before executing
            context.record_tool_call(&tool_name, &tool_args);

            debug!("Executing tool: {}", tool_name);

            let result = match self
                .tools
                .execute_call(&tool_call.function, &tool_context)
                .await
            {
                Ok(exec_result) => {
                    // Reset circuit-breaker on success
                    self.tools.reset_failure(&tool_call.function.name);
                    let tool_result = exec_result.to_tool_result(&tool_call.id);
                    // Extract artifacts from successful tool results
                    self.extract_and_store_artifacts(
                        context.id(),
                        &tool_result.content,
                        &tool_call.function.name,
                    );
                    info!("Tool {} executed successfully", tool_call.function.name);
                    tool_result
                }
                Err(e) => {
                    // Record failure for circuit-breaker
                    self.tools.record_failure(&tool_call.function.name);
                    error!("Tool {} failed: {}", tool_call.function.name, e);
                    ToolResult::error(&tool_call.id, format!("Tool execution failed: {}", e))
                }
            };

            results.push(result);
        }

        // Add tool results to context
        for result in results {
            context.add_message(Message {
                role: Role::Tool,
                content: result.content,
                reasoning_content: None,
                name: None,
                tool_calls: None,
                tool_call_id: Some(result.tool_call_id),
                metadata: None,
            });
        }

        // Check execution controller before next iteration
        {
            let ctrl_guard = self.execution_controller.read().await;
            if let Some(ref ctrl) = *ctrl_guard {
                if let Err(reason) = ctrl.check_and_wait().await {
                    return Ok(crate::providers::CompletionResponse {
                        message: Message {
                            role: Role::Assistant,
                            content: format!("Execution halted: {}", reason),
                            reasoning_content: None,
                            name: None,
                            tool_calls: None,
                            tool_call_id: None,
                            metadata: None,
                        },
                        usage: None,
                        model: "system".to_string(),
                        finish_reason: Some("cancelled".to_string()),
                    });
                }
            }
        }

        // Get final response (boxed to avoid recursive async issue)
        Box::pin(self.get_completion(context)).await
    }

    /// Get a completion from the LLM with progress callbacks
    async fn get_completion_with_progress(
        &self,
        context: &mut Context,
        progress_cb: ProgressCallback,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        let messages = context.to_messages();

        // Get available tools
        let tool_context = self.build_tool_context("user", context.id());
        let tool_defs = self.tools.get_available(&tool_context);
        let has_tools = !tool_defs.is_empty();

        let extra = self.extra_params.read().await.clone();
        let mut request = CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            stream: true,
            extra,
            ..Default::default()
        };
        self.patch_request_for_reasoning(&mut request);

        if has_tools && self.provider.supports_tools() {
            let tools: Vec<crate::providers::ToolDefinition> = tool_defs
                .into_iter()
                .map(|f| crate::providers::ToolDefinition {
                    tool_type: "function".to_string(),
                    function: f,
                })
                .collect();
            request.tools = Some(tools);
        }

        // Check live cost guard before calling provider
        if let Some(ref guard) = self.cost_guard {
            if guard.is_exceeded() {
                return Err(crate::error::MantaError::Validation(
                    "Budget limit exceeded — refusing provider call. \
                     Adjust daily_limit_cents or hourly_action_limit in config."
                        .to_string(),
                ));
            }
        }

        // Notify generating (starting)
        (progress_cb)(ProgressEvent::Generating { content: None }).await;

        // Get streaming completion — use model router when available
        let (raw_stream, family) = if let Some(ref router) = self.model_router {
            let alias = {
                let guard = self.model_override.read().await;
                guard
                    .as_ref()
                    .cloned()
                    .or(self.model_alias.clone())
                    .or(self.model.clone())
                    .unwrap_or_else(|| self.provider.default_model().to_string())
            };
            let stream = router.stream(&alias, request.messages).await?;
            // When using model router, fall back to Generic stream family
            (stream, crate::providers::stream_wrappers::ProviderStreamFamily::Generic)
        } else {
            (self.provider.stream(request).await?, self.provider.stream_family())
        };
        let registry = crate::providers::stream_wrappers::StreamFamilyRegistry::default();
        let mut stream = registry.apply(family, raw_stream);

        let mut accumulated_text = String::new();
        let mut accumulated_reasoning = String::new();
        let mut accumulated_tool_calls: Vec<ToolCall> = Vec::new();
        let mut finish_reason: Option<String> = None;
        let mut usage: Option<crate::providers::Usage> = None;

        while let Some(chunk) = stream.next().await {
            // Emit reasoning delta
            if let Some(ref reasoning_delta) = chunk.reasoning_content {
                if !reasoning_delta.is_empty() {
                    accumulated_reasoning.push_str(reasoning_delta);
                    (progress_cb)(ProgressEvent::Generating {
                        content: Some(reasoning_delta.clone()),
                    })
                    .await;
                }
            }

            // Emit text delta
            if let Some(ref text_delta) = chunk.content {
                if !text_delta.is_empty() {
                    accumulated_text.push_str(text_delta);
                    (progress_cb)(ProgressEvent::ContentDelta { text: text_delta.clone() }).await;
                }
            }

            // Accumulate tool calls from stream
            if let Some(ref calls) = chunk.tool_calls {
                for call in calls {
                    // Merge partial tool calls by index (streaming deltas use index as key)
                    let key = call.index.unwrap_or(0);
                    if let Some(existing) = accumulated_tool_calls
                        .iter_mut()
                        .find(|c| c.index == Some(key) || (c.index.is_none() && c.id == call.id))
                    {
                        // Fill in id/type/name from first chunk if they were empty
                        if existing.id.is_empty() && !call.id.is_empty() {
                            existing.id = call.id.clone();
                        }
                        if existing.call_type.is_empty() && !call.call_type.is_empty() {
                            existing.call_type = call.call_type.clone();
                        }
                        existing.function.name.push_str(&call.function.name);
                        existing
                            .function
                            .arguments
                            .push_str(&call.function.arguments);
                    } else {
                        accumulated_tool_calls.push(call.clone());
                    }
                }
            }

            if chunk.is_done {
                finish_reason = Some("stop".to_string());
                usage = chunk.usage;
                break;
            }
        }

        // Build the final message
        let final_message = Message {
            role: Role::Assistant,
            content: accumulated_text.clone(),
            reasoning_content: if accumulated_reasoning.is_empty() {
                None
            } else {
                Some(accumulated_reasoning.clone())
            },
            name: None,
            tool_calls: if accumulated_tool_calls.is_empty() {
                None
            } else {
                Some(accumulated_tool_calls.clone())
            },
            tool_call_id: None,
            metadata: None,
        };

        let response = crate::providers::CompletionResponse {
            message: final_message,
            usage,
            model: self
                .model
                .clone()
                .unwrap_or_else(|| self.provider.default_model().to_string()),
            finish_reason,
        };

        // Record token usage in cost guard (approximate from accumulated text if no usage provided)
        if let Some(ref guard) = self.cost_guard {
            let prompt_tokens = context
                .to_messages()
                .iter()
                .map(|m| m.content.len() / 4)
                .sum::<usize>() as u64;
            let completion_tokens =
                (accumulated_text.len() + accumulated_reasoning.len()) as u64 / 4;
            guard.record_usage(prompt_tokens, completion_tokens, response.model.as_str());
        }

        // Handle tool calls if present
        if let Some(ref tool_calls) = response.message.tool_calls {
            if !tool_calls.is_empty() {
                debug!("Processing {} tool calls with progress", tool_calls.len());
                return self
                    .handle_tool_calls_with_progress(context, &response, tool_calls, progress_cb)
                    .await;
            }
        }

        // Add assistant message to context
        context.add_message(response.message.clone());

        Ok(response)
    }

    /// Handle tool calls with progress callbacks
    async fn handle_tool_calls_with_progress(
        &self,
        context: &mut Context,
        original_response: &crate::providers::CompletionResponse,
        tool_calls: &[ToolCall],
        progress_cb: ProgressCallback,
    ) -> crate::Result<crate::providers::CompletionResponse> {
        // Check iteration limit before processing
        if !context.increment_tool_iteration() {
            warn!("Tool iteration limit reached ({}), stopping", context.tool_iterations());

            // Notify user about the limit
            (progress_cb)(ProgressEvent::Error {
                message: format!("Tool iteration limit reached ({}) - the agent was taking too many steps. Please try a more specific request.", Context::DEFAULT_MAX_TOOL_ITERATIONS),
            }).await;

            // Return a response indicating the limit was reached
            return Ok(crate::providers::CompletionResponse {
                message: Message {
                    role: Role::Assistant,
                    content: format!("I've reached the maximum number of tool calls ({}) for this request. The task may be too complex or the tools may not be providing the expected results. Please try a more specific request or break the task into smaller steps.", Context::DEFAULT_MAX_TOOL_ITERATIONS),
                    reasoning_content: None,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    metadata: None,
                },
                usage: None,
                model: "system".to_string(),
                finish_reason: Some("tool_limit".to_string()),
            });
        }

        // Add assistant message with tool calls
        context.add_message(original_response.message.clone());

        // Execute tools with progress
        let tool_context = self
            .build_tool_context("user", context.id())
            .with_timeout(std::time::Duration::from_secs(30));

        let mut results = Vec::new();

        for tool_call in tool_calls.iter().take(self.config.max_concurrent_tools) {
            let tool_name = tool_call.function.name.clone();
            let tool_args = tool_call.function.arguments.clone();

            // Check for duplicate tool calls
            if context.is_tool_call_duplicate(&tool_name, &tool_args) {
                warn!("Duplicate tool call detected: {} with same args, skipping", tool_name);

                // Notify about duplicate
                (progress_cb)(ProgressEvent::ToolResult {
                    name: tool_name.clone(),
                    result: "[Duplicate tool call skipped - already executed with same parameters]"
                        .to_string(),
                })
                .await;

                // Add error result so LLM knows this failed
                results.push(ToolResult::error(
                    &tool_call.id,
                    "Error: This exact tool call was already executed. The previous result did not provide the expected data. Please try a different approach or acknowledge that the tool cannot fulfill this request."
                ));
                continue;
            }

            // Record this tool call before executing
            context.record_tool_call(&tool_name, &tool_args);

            // Notify tool calling
            (progress_cb)(ProgressEvent::ToolCalling {
                name: tool_name.clone(),
                arguments: tool_args,
            })
            .await;

            debug!("Executing tool: {}", tool_name);

            let result = match self
                .tools
                .execute_call(&tool_call.function, &tool_context)
                .await
            {
                Ok(exec_result) => {
                    // Reset circuit-breaker on success
                    self.tools.reset_failure(&tool_name);
                    let tool_result = exec_result.to_tool_result(&tool_call.id);
                    let result_str = tool_result.content.clone();

                    // Extract artifacts from successful tool results
                    self.extract_and_store_artifacts(context.id(), &result_str, &tool_name);

                    // Notify tool result
                    (progress_cb)(ProgressEvent::ToolResult {
                        name: tool_name.clone(),
                        result: result_str.chars().take(200).collect(), // Truncate for display
                    })
                    .await;

                    info!("Tool {} executed successfully", tool_name);
                    tool_result
                }
                Err(e) => {
                    // Record failure for circuit-breaker
                    self.tools.record_failure(&tool_name);
                    let error_msg = format!("Tool execution failed: {}", e);

                    // Notify tool error
                    (progress_cb)(ProgressEvent::ToolResult {
                        name: tool_name.clone(),
                        result: error_msg.clone(),
                    })
                    .await;

                    error!("Tool {} failed: {}", tool_name, e);
                    ToolResult::error(&tool_call.id, error_msg)
                }
            };

            results.push(result);
        }

        // Build a map of tool_call_id -> result content for history persistence
        let tool_result_map: std::collections::HashMap<String, String> = results
            .iter()
            .map(|r| (r.tool_call_id.clone(), r.content.clone()))
            .collect();

        // Add tool results to context
        for result in results {
            context.add_message(Message {
                role: Role::Tool,
                content: result.content,
                reasoning_content: None,
                name: None,
                tool_calls: None,
                tool_call_id: Some(result.tool_call_id),
                metadata: None,
            });
        }

        // Check execution controller before next iteration
        {
            let ctrl_guard = self.execution_controller.read().await;
            if let Some(ref ctrl) = *ctrl_guard {
                if let Err(reason) = ctrl.check_and_wait().await {
                    // Notify cancellation
                    (progress_cb)(ProgressEvent::Error {
                        message: format!("Execution halted: {}", reason),
                    })
                    .await;

                    return Ok(crate::providers::CompletionResponse {
                        message: Message {
                            role: Role::Assistant,
                            content: format!("Execution halted: {}", reason),
                            reasoning_content: None,
                            name: None,
                            tool_calls: None,
                            tool_call_id: None,
                            metadata: None,
                        },
                        usage: None,
                        model: "system".to_string(),
                        finish_reason: Some("cancelled".to_string()),
                    });
                }
            }
        }

        // Get final response with progress
        let mut final_response =
            Box::pin(self.get_completion_with_progress(context, progress_cb)).await?;

        // Preserve tool calls from the original assistant message so that
        // downstream consumers (session_store, etc.) can see what tools were invoked.
        if let Some(ref original_calls) = original_response.message.tool_calls {
            match final_response.message.tool_calls {
                None => final_response.message.tool_calls = Some(original_calls.clone()),
                Some(ref mut existing) => {
                    let mut merged = original_calls.clone();
                    merged.append(existing);
                    final_response.message.tool_calls = Some(merged);
                }
            }
        }

        // Attach execution results to the preserved tool calls so that history
        // replay can show "Done" instead of "Running".
        if let Some(ref mut calls) = final_response.message.tool_calls {
            for call in calls.iter_mut() {
                if call.result.is_none() {
                    if let Some(result_content) = tool_result_map.get(&call.id) {
                        call.result = Some(result_content.clone());
                    }
                }
            }
        }

        Ok(final_response)
    }

    /// Start the agent (for background processing if needed)
    pub async fn start(&self) -> crate::Result<()> {
        info!("Starting agent");
        // Agent is mostly stateless, but this could be used for background tasks
        Ok(())
    }

    /// Spawn a background self-repair task.
    ///
    /// Every `check_interval` the task:
    /// 1. Evicts contexts that have been inactive longer than `stale_threshold`.
    /// 2. Logs and reports any tools that are currently circuit-broken.
    ///
    /// The task runs until the `Agent` is dropped.
    pub fn start_self_repair_loop(
        &self,
        check_interval: Duration,
        stale_threshold: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let thread_map = Arc::clone(&self.thread_map);
        let tools = Arc::clone(&self.tools);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(check_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                ticker.tick().await;

                // ── 1. Evict stale threads ────────────────────────────────────
                let stale_ids: Vec<String> = {
                    let guard = thread_map.lock().await;
                    guard
                        .iter()
                        .filter(|(_, t)| t.context.is_stale(stale_threshold))
                        .map(|(id, _)| id.clone())
                        .collect()
                };

                if !stale_ids.is_empty() {
                    let mut guard = thread_map.lock().await;
                    for id in &stale_ids {
                        guard.remove(id);
                        warn!(
                            conversation_id = id.as_str(),
                            "Self-repair: evicted stale context (inactive >{:?})", stale_threshold
                        );
                    }
                }

                // ── 2. Report degraded tools ──────────────────────────────────
                let degraded = tools.degraded_tools();
                if !degraded.is_empty() {
                    warn!(
                        tools = ?degraded,
                        "Self-repair: {} tool(s) are circuit-broken",
                        degraded.len()
                    );
                }
            }
        })
    }

    /// Return a summary of all active threads:
    /// `(thread_id, label, turn_count, conversation_id)`.
    pub async fn thread_summaries(&self) -> Vec<(String, String, usize, String)> {
        let map = self.thread_map.lock().await;
        map.iter()
            .map(|(conv_id, t)| (t.id.clone(), t.label.clone(), t.turns.len(), conv_id.clone()))
            .collect()
    }

    /// Return turn details for a conversation, identified by its
    /// `conversation_id` (the `thread_map` key).
    ///
    /// Each element is `(index, state_str, user_preview, asst_preview)`.
    /// Returns `None` if no thread exists for that conversation.
    pub async fn thread_turns_for(
        &self,
        conv_id: &str,
    ) -> Option<Vec<(usize, String, String, String)>> {
        let map = self.thread_map.lock().await;
        map.get(conv_id).map(|t| {
            t.turns
                .iter()
                .map(|turn| {
                    let state = format!("{:?}", turn.state).to_lowercase();
                    let user_preview: String = turn.user_message.chars().take(80).collect();
                    let asst_preview: String = turn.assistant_response.chars().take(80).collect();
                    (turn.index, state, user_preview, asst_preview)
                })
                .collect()
        })
    }

    /// Return context assembly info for a conversation.
    ///
    /// Returns `(message_count, token_count, max_tokens, system_prompt_len, tool_iterations)`
    /// or `None` if the thread is not found.
    pub async fn context_info(&self, conv_id: &str) -> Option<(usize, usize, usize, usize, usize)> {
        let map = self.thread_map.lock().await;
        map.get(conv_id).map(|t| {
            (
                t.context.message_count(),
                t.context.token_count(),
                t.context.max_context_tokens(),
                t.context.system_prompt().len(),
                t.context.tool_iterations(),
            )
        })
    }

    /// Compact the context for a conversation using the Summarize strategy.
    ///
    /// Returns `(before_message_count, after_message_count)` or `None` if the
    /// thread is not found or no compaction was needed.
    pub async fn compact_context(&self, conv_id: &str) -> Option<(usize, usize)> {
        let mut map = self.thread_map.lock().await;
        map.get_mut(conv_id).map(|thread| {
            let messages = thread.context.to_messages();
            let before = messages.len();
            let target = thread.context.max_context_tokens() / 2;
            let compressor =
                ContextCompressor::new(target).with_strategy(CompressionStrategy::Summarize);
            let compressed = compressor.compress(&messages);
            let after = compressed.len();
            if after < before {
                thread.context.replace_messages(compressed);
            }
            (before, after)
        })
    }

    /// Undo the last turn for a conversation.
    ///
    /// Moves the most recent `Turn` from the turn log to the redo stack and
    /// strips the corresponding messages from the context window. If a
    /// `SessionStore` is attached the turn rows are also hard-deleted from
    /// SQLite (fire-and-forget).
    ///
    /// Returns `true` if a turn was undone, `false` if the thread was empty or
    /// not found.
    pub async fn undo_last_turn(&self, conversation_id: &str) -> bool {
        let mut map = self.thread_map.lock().await;
        if let Some(thread) = map.get_mut(conversation_id) {
            let last_idx = thread.turns.len().saturating_sub(1) as i64;
            let undone = thread.undo_last_turn();
            if undone {
                if let (Some(store), Some(sid)) =
                    (self.session_store.clone(), self.session_id.clone())
                {
                    let tid = thread.id.clone();
                    tokio::spawn(async move {
                        let _ = store.delete_turn(&sid, &tid, last_idx).await;
                    });
                }
            }
            undone
        } else {
            false
        }
    }

    /// Redo the most recently undone turn for a conversation.
    ///
    /// Restores the turn from the redo stack back to the turn log and
    /// re-inserts its messages into the context window. Note: persistence
    /// is not supported for redo (the turn was deleted from SQLite on undo).
    ///
    /// Returns `true` if a turn was redone, `false` if the redo stack was empty
    /// or the thread was not found.
    pub async fn redo_last_turn(&self, conversation_id: &str) -> bool {
        let mut map = self.thread_map.lock().await;
        if let Some(thread) = map.get_mut(conversation_id) {
            thread.redo_last_turn()
        } else {
            false
        }
    }

    /// Returns `true` if the conversation can undo a turn.
    pub async fn can_undo(&self, conversation_id: &str) -> bool {
        let map = self.thread_map.lock().await;
        map.get(conversation_id)
            .map(|t| t.can_undo())
            .unwrap_or(false)
    }

    /// Returns `true` if the conversation can redo a turn.
    pub async fn can_redo(&self, conversation_id: &str) -> bool {
        let map = self.thread_map.lock().await;
        map.get(conversation_id)
            .map(|t| t.can_redo())
            .unwrap_or(false)
    }

    /// Restore threads from the `SessionStore` for the current `session_id`.
    ///
    /// This rebuilds each persisted `Thread` (system prompt + accumulated
    /// history) so conversation continuity survives a restart.  Call once
    /// during agent startup, after `with_session_store` has been configured.
    pub async fn restore_threads(&self) -> crate::Result<()> {
        let store = self
            .session_store
            .as_ref()
            .ok_or_else(|| crate::error::MantaError::Internal("no session store".into()))?;
        let sid = self
            .session_id
            .as_deref()
            .ok_or_else(|| crate::error::MantaError::Internal("no session id".into()))?;

        let thread_rows = store.load_threads_for_session(sid).await?;
        let mut map = self.thread_map.lock().await;

        for (tid, label, _created_ms, turns) in thread_rows {
            // Build a fresh context (system prompt, token limits) — history
            // is replayed via push_turn / complete below.
            let ctx = self.build_fresh_context(&tid, "restore", "").await;
            let mut thread = Thread::from_context(&tid, &label, ctx);
            for (_idx, user_msg, asst_msg, _state) in turns {
                let i = thread.push_turn(&user_msg);
                thread.context.add_message(Message::user(&user_msg));
                thread.turns[i].complete(asst_msg.clone());
                thread.context.add_message(Message::assistant(&asst_msg));
            }
            // Thread is keyed by conversation_id; the thread_id is "thread-{conv_id}".
            let conv_id = tid.trim_start_matches("thread-").to_string();
            map.insert(conv_id, thread);
        }

        info!("Restored {} thread(s) from session {}", map.len(), sid);
        Ok(())
    }

    /// Close a conversation and trigger compaction if eligible.
    ///
    /// Compaction is triggered when the session has accumulated more than 50
    /// turns OR is older than 7 days. Compaction extracts key facts from the
    /// conversation history into semantic memories via the MemoryManager.
    ///
    /// The thread is removed from `thread_map` regardless of compaction.
    /// Also flushes transcript and cleans up session files.
    pub async fn close_conversation(&self, conversation_id: &str) {
        const MAX_TURNS_BEFORE_COMPACT: usize = 50;
        const MAX_AGE_DAYS: u64 = 7;

        // Remove the thread from the map
        let thread_opt = {
            let mut map = self.thread_map.lock().await;
            map.remove(conversation_id)
        };

        let thread = match thread_opt {
            Some(t) => t,
            None => return, // Nothing to close
        };

        // Flush transcript to disk
        if let Some(ref transcript_store) = self.transcript_store {
            if let Err(e) = transcript_store.flush(conversation_id) {
                warn!("Failed to flush transcript for {}: {}", conversation_id, e);
            } else {
                info!("Flushed transcript for {}", conversation_id);
            }
        }

        // Cleanup session files
        if let Some(ref file_manager) = self.session_file_manager {
            if let Err(e) = file_manager.cleanup_session(conversation_id).await {
                warn!("Failed to cleanup session files for {}: {}", conversation_id, e);
            } else {
                info!("Cleaned up session files for {}", conversation_id);
            }
        }

        // Clear disk budget tracking for this session
        if let Some(ref budget) = self.disk_budget {
            budget.clear_session(conversation_id);
        }

        // Determine if compaction is needed
        let age_secs = thread.created_at.elapsed().unwrap_or_default().as_secs();
        let too_old = age_secs > MAX_AGE_DAYS * 86_400;
        let too_long = thread.turn_count() > MAX_TURNS_BEFORE_COMPACT;

        if too_old || too_long {
            if let Some(mm) = self.memory_manager.clone() {
                let conv_id = conversation_id.to_string();
                tokio::spawn(async move {
                    match mm.compact_session(&conv_id, None).await {
                        Ok(ids) => {
                            info!("Session {} compacted: {} facts extracted", conv_id, ids.len());
                        }
                        Err(e) => {
                            warn!("Session compaction failed for {}: {}", conv_id, e);
                        }
                    }
                });
            }
        } else {
            debug!(
                "Session {} closed without compaction ({} turns, {} days old)",
                conversation_id,
                thread.turn_count(),
                age_secs / 86_400
            );
        }
    }

    /// Shutdown the agent, compacting all active sessions.
    pub async fn shutdown(&self) -> crate::Result<()> {
        info!("Shutting down agent");

        // Compact all open sessions before shutting down
        let conversation_ids: Vec<String> = {
            let map = self.thread_map.lock().await;
            map.keys().cloned().collect()
        };
        for conv_id in conversation_ids {
            self.close_conversation(&conv_id).await;
        }

        if let Some(tx) = self.shutdown_tx.write().await.take() {
            let _ = tx.send(()).await;
        }
        Ok(())
    }

    /// Get agent health status
    pub async fn health_check(&self) -> crate::Result<bool> {
        self.provider.health_check().await
    }

    /// Get the tool registry
    pub fn get_tools(&self) -> &ToolRegistry {
        &self.tools
    }

    /// Extract artifacts (code blocks, links) from tool result content
    /// and store them in the artifact store.
    fn extract_and_store_artifacts(&self, session_id: &str, content: &str, tool_name: &str) {
        use regex::Regex;

        let Some(ref artifact_store) = self.artifact_store else {
            return;
        };

        // Extract code blocks: ```language\ncode\n```
        let code_block_re = Regex::new(r"```(\w+)?\n(.*?)\n```").expect("valid code block regex");
        for (idx, cap) in code_block_re.captures_iter(content).enumerate() {
            let language = cap.get(1).map(|m| m.as_str()).unwrap_or("text");
            let code = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            if code.len() < 20 {
                continue; // Skip trivial snippets
            }
            let artifact = Artifact::code(
                format!("{}-code-{}", tool_name, idx),
                session_id,
                format!("Code from {} ({})", tool_name, language),
                language,
                code,
            );
            let size = artifact.size_bytes;
            artifact_store.add(artifact);
            // Track in disk budget
            if let Some(ref budget) = self.disk_budget {
                let _ = budget.track_item(
                    session_id,
                    format!("artifact-{}-code-{}", tool_name, idx),
                    BudgetCategory::Artifact,
                    size,
                );
            }
        }

        // Extract URLs/links
        let url_re = Regex::new(r#"https?://[^\s)\]>'"`]+"#).expect("valid url regex");
        for (idx, cap) in url_re.captures_iter(content).enumerate() {
            let url = cap.get(0).map(|m| m.as_str()).unwrap_or("");
            if url.len() < 10 {
                continue;
            }
            let artifact = Artifact::link(
                format!("{}-link-{}", tool_name, idx),
                session_id,
                format!("Link from {}", tool_name),
                url,
            );
            let size = artifact.size_bytes;
            artifact_store.add(artifact);
            if let Some(ref budget) = self.disk_budget {
                let _ = budget.track_item(
                    session_id,
                    format!("artifact-{}-link-{}", tool_name, idx),
                    BudgetCategory::Artifact,
                    size,
                );
            }
        }
    }
}

/// Builder for Agent
#[derive(Default)]
pub struct AgentBuilder {
    config: Option<AgentConfig>,
    provider: Option<Arc<dyn Provider>>,
    tools: Option<Arc<ToolRegistry>>,
    memory_store: Option<Arc<dyn crate::memory::MemoryStore>>,
    chat_history: Option<Arc<dyn crate::memory::ChatHistoryStore>>,
    session_search: Option<Arc<crate::memory::SessionSearch>>,
    transcript_store: Option<Arc<crate::agent::TranscriptStore>>,
    artifact_store: Option<Arc<crate::agent::ArtifactStore>>,
    disk_budget: Option<Arc<crate::agent::DiskBudgetManager>>,
    session_file_manager: Option<Arc<crate::agent::SessionFileManager>>,
    model_router: Option<Arc<crate::model_router::ModelRouter>>,
    model_alias: Option<String>,
    skill_manager: Option<Arc<tokio::sync::RwLock<crate::skills::SkillManager>>>,
}

impl AgentBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set configuration
    pub fn config(mut self, config: AgentConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Set skills prompt
    pub fn skills(mut self, skills_prompt: String) -> Self {
        let mut config = self.config.unwrap_or_default();
        config.skills_prompt = Some(skills_prompt);
        self.config = Some(config);
        self
    }

    /// Set provider
    pub fn provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Set tools
    pub fn tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Set memory store for persistent memory
    pub fn memory_store(mut self, store: Arc<dyn crate::memory::MemoryStore>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Set chat history store for conversation persistence
    pub fn chat_history(mut self, store: Arc<dyn crate::memory::ChatHistoryStore>) -> Self {
        self.chat_history = Some(store);
        self
    }

    /// Set session search for conversation indexing
    pub fn session_search(mut self, search: Arc<crate::memory::SessionSearch>) -> Self {
        self.session_search = Some(search);
        self
    }

    /// Set transcript store for conversation recording
    pub fn transcript_store(mut self, store: Arc<crate::agent::TranscriptStore>) -> Self {
        self.transcript_store = Some(store);
        self
    }

    /// Set artifact store for session-bound artifacts
    pub fn artifact_store(mut self, store: Arc<crate::agent::ArtifactStore>) -> Self {
        self.artifact_store = Some(store);
        self
    }

    /// Set disk budget manager for per-session storage quota
    pub fn disk_budget(mut self, budget: Arc<crate::agent::DiskBudgetManager>) -> Self {
        self.disk_budget = Some(budget);
        self
    }

    /// Set session file manager for isolated per-session file ops
    pub fn session_file_manager(mut self, manager: Arc<crate::agent::SessionFileManager>) -> Self {
        self.session_file_manager = Some(manager);
        self
    }

    /// Set model router for advanced routing, key rotation, and fallback.
    pub fn model_router(mut self, router: Arc<crate::model_router::ModelRouter>) -> Self {
        self.model_router = Some(router);
        self
    }

    /// Set model alias used when routing through the model router.
    pub fn model_alias(mut self, alias: impl Into<String>) -> Self {
        self.model_alias = Some(alias.into());
        self
    }

    /// Set skill manager for dynamic skill injection.
    pub fn skill_manager(
        mut self,
        manager: Arc<tokio::sync::RwLock<crate::skills::SkillManager>>,
    ) -> Self {
        self.skill_manager = Some(manager);
        self
    }

    /// Build the agent
    pub fn build(self) -> crate::Result<Agent> {
        let mut agent = Agent::new(
            self.config.unwrap_or_default(),
            self.provider.ok_or_else(|| {
                crate::error::MantaError::Validation("Provider required".to_string())
            })?,
            self.tools.unwrap_or_else(|| Arc::new(ToolRegistry::new())),
        );

        if let Some(store) = self.memory_store {
            agent = agent.with_memory_store(store);
        }

        if let Some(store) = self.chat_history {
            agent = agent.with_chat_history(store);
        }

        if let Some(search) = self.session_search {
            agent = agent.with_session_search(search);
        }

        if let Some(store) = self.transcript_store {
            agent = agent.with_transcript_store(store);
        }

        if let Some(store) = self.artifact_store {
            agent = agent.with_artifact_store(store);
        }

        if let Some(budget) = self.disk_budget {
            agent = agent.with_disk_budget(budget);
        }

        if let Some(manager) = self.session_file_manager {
            agent = agent.with_session_file_manager(manager);
        }

        if let Some(router) = self.model_router {
            agent = agent.with_model_router(router);
        }

        if let Some(alias) = self.model_alias {
            agent = agent.with_model_alias(alias);
        }

        if let Some(manager) = self.skill_manager {
            agent = agent.with_skill_manager(manager);
        }

        Ok(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{CompletionResponse, CompletionStream};
    use std::path::PathBuf;

    // ── Mock Provider ─────────────────────────────────────────────────────────

    struct MockProvider;

    #[async_trait::async_trait]
    impl crate::providers::Provider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        fn default_model(&self) -> &str {
            "mock-model"
        }
        fn supports_tools(&self) -> bool {
            false
        }
        fn max_context(&self) -> usize {
            4096
        }
        async fn complete(&self, _req: CompletionRequest) -> crate::Result<CompletionResponse> {
            Ok(CompletionResponse {
                message: Message::assistant("hello"),
                usage: None,
                model: "mock-model".to_string(),
                finish_reason: None,
            })
        }
        async fn stream(&self, _req: CompletionRequest) -> crate::Result<CompletionStream> {
            Ok(Box::pin(tokio_stream::iter(vec![])))
        }
        async fn health_check(&self) -> crate::Result<bool> {
            Ok(true)
        }
    }

    // ── is_obviously_time_sensitive ───────────────────────────────────────────

    #[test]
    fn test_is_obviously_time_sensitive_positive() {
        assert!(is_obviously_time_sensitive("what time is it now"));
        assert!(is_obviously_time_sensitive("CURRENT TIME please"));
        assert!(is_obviously_time_sensitive("what's the time in Tokyo"));
        assert!(is_obviously_time_sensitive("现在几点了"));
        assert!(is_obviously_time_sensitive("当前时间是多少"));
        assert!(is_obviously_time_sensitive("现在时间呢"));
    }

    #[test]
    fn test_is_obviously_time_sensitive_negative() {
        assert!(!is_obviously_time_sensitive("hello"));
        assert!(!is_obviously_time_sensitive("what is the weather"));
        assert!(!is_obviously_time_sensitive("explain quantum computing"));
        assert!(!is_obviously_time_sensitive("what time zone is EST"));
    }

    // ── are_tools_cacheable ───────────────────────────────────────────────────

    #[test]
    fn test_are_tools_cacheable_all_cacheable() {
        assert!(are_tools_cacheable(&["search".to_string(), "read_file".to_string()]));
        assert!(are_tools_cacheable(&[]));
        assert!(are_tools_cacheable(&["grep".to_string()]));
    }

    #[test]
    fn test_are_tools_cacheable_non_cacheable() {
        assert!(!are_tools_cacheable(&["datetime".to_string()]));
        assert!(!are_tools_cacheable(&["time".to_string(), "read_file".to_string()]));
        assert!(!are_tools_cacheable(&["weather_current".to_string()]));
        assert!(!are_tools_cacheable(&["stock_price".to_string()]));
        assert!(!are_tools_cacheable(&["crypto_price".to_string()]));
        assert!(!are_tools_cacheable(&["my_clock_tool".to_string()]));
        assert!(!are_tools_cacheable(&["get_date_today".to_string()]));
    }

    // ── AgentConfig ───────────────────────────────────────────────────────────

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.max_context_tokens, 4096);
        assert_eq!(config.max_concurrent_tools, 5);
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.max_tokens, 2048);
        assert!(config.skills_prompt.is_none());
        assert!(config.max_turns.is_none());
        assert!(config.compaction_model.is_none());
        assert!(!config.system_prompt.is_empty());
    }

    #[test]
    fn test_agent_config_full_system_prompt_without_skills() {
        let config = AgentConfig {
            system_prompt: "base".to_string(),
            ..Default::default()
        };
        assert_eq!(config.full_system_prompt(), "base");
    }

    #[test]
    fn test_agent_config_full_system_prompt_with_skills() {
        let config = AgentConfig {
            system_prompt: "base".to_string(),
            skills_prompt: Some("skill1".to_string()),
            ..Default::default()
        };
        let prompt = config.full_system_prompt();
        assert!(prompt.contains("base"));
        assert!(prompt.contains("skill1"));
        assert!(prompt.contains("Skills"));
    }

    #[test]
    fn test_agent_config_serde_roundtrip() {
        let config = AgentConfig {
            system_prompt: "test prompt".to_string(),
            max_context_tokens: 8192,
            max_concurrent_tools: 3,
            temperature: 0.5,
            max_tokens: 1024,
            skills_prompt: Some("skills".to_string()),
            max_turns: Some(10),
            compaction_model: Some("claude-haiku".to_string()),
            workspace_dir: None,
            workspace_only: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: AgentConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.max_context_tokens, 8192);
        assert_eq!(restored.temperature, 0.5);
        assert_eq!(restored.max_tokens, 1024);
        assert_eq!(restored.max_concurrent_tools, 3);
        assert_eq!(restored.max_turns, Some(10));
        assert_eq!(restored.compaction_model, Some("claude-haiku".to_string()));
    }

    // ── ResponseCache ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_response_cache_empty() {
        let cache = ResponseCache::new(Duration::from_secs(60));
        let result = cache.get("user1", "conv1", "hello").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_response_cache_set_and_get() {
        let cache = ResponseCache::new(Duration::from_secs(60));
        cache
            .set("user1", "conv1", "hello", "world".to_string(), vec![])
            .await;
        let result = cache.get("user1", "conv1", "hello").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().response, "world");
    }

    #[tokio::test]
    async fn test_response_cache_key_isolation() {
        let cache = ResponseCache::new(Duration::from_secs(60));
        cache
            .set("user1", "conv1", "hello", "world".to_string(), vec![])
            .await;
        // Different user
        assert!(cache.get("user2", "conv1", "hello").await.is_none());
        // Different conversation
        assert!(cache.get("user1", "conv2", "hello").await.is_none());
        // Different message
        assert!(cache.get("user1", "conv1", "bye").await.is_none());
    }

    #[tokio::test]
    async fn test_response_cache_ttl_expiration() {
        let cache = ResponseCache::new(Duration::from_millis(50));
        cache
            .set("user1", "conv1", "hello", "world".to_string(), vec![])
            .await;
        // Immediately available
        assert!(cache.get("user1", "conv1", "hello").await.is_some());
        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(cache.get("user1", "conv1", "hello").await.is_none());
    }

    #[tokio::test]
    async fn test_response_cache_cleanup() {
        let cache = ResponseCache::new(Duration::from_millis(50));
        cache
            .set("user1", "conv1", "hello", "world".to_string(), vec![])
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        cache.cleanup().await;
        // After cleanup, entry should be gone
        let cache_guard = cache.cache.read().await;
        assert!(cache_guard.is_empty());
    }

    #[test]
    fn test_response_cache_generate_key_consistency() {
        let key1 = ResponseCache::generate_key("u1", "c1", "hello");
        let key2 = ResponseCache::generate_key("u1", "c1", "hello");
        let key3 = ResponseCache::generate_key("u1", "c1", "hello ");
        assert_eq!(key1, key2);
        // Trimmed so trailing space doesn't matter
        assert_eq!(key1, key3);
    }

    #[test]
    fn test_response_cache_generate_key_uniqueness() {
        let key1 = ResponseCache::generate_key("u1", "c1", "hello");
        let key2 = ResponseCache::generate_key("u1", "c1", "world");
        let key3 = ResponseCache::generate_key("u2", "c1", "hello");
        assert_ne!(key1, key2);
        assert_ne!(key1, key3);
    }

    // ── AgentBuilder ──────────────────────────────────────────────────────────

    #[test]
    fn test_agent_builder_new() {
        let builder = AgentBuilder::new();
        assert!(builder.config.is_none());
        assert!(builder.provider.is_none());
        assert!(builder.tools.is_none());
    }

    #[test]
    fn test_agent_builder_default() {
        let builder: AgentBuilder = Default::default();
        assert!(builder.config.is_none());
    }

    #[test]
    fn test_agent_builder_chaining() {
        let builder = AgentBuilder::new()
            .config(AgentConfig::default())
            .skills("skill1".to_string())
            .provider(Arc::new(MockProvider))
            .tools(Arc::new(ToolRegistry::new()));
        assert!(builder.config.is_some());
        assert!(builder.provider.is_some());
        assert!(builder.tools.is_some());
    }

    #[test]
    fn test_agent_builder_build_without_provider_fails() {
        let builder = AgentBuilder::new().config(AgentConfig::default());
        let result = builder.build();
        match result {
            Err(e) => assert!(e.to_string().contains("Provider required")),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn test_agent_builder_build_success() {
        let builder = AgentBuilder::new()
            .config(AgentConfig::default())
            .provider(Arc::new(MockProvider))
            .tools(Arc::new(ToolRegistry::new()));
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_agent_builder_skills_prompt() {
        let builder = AgentBuilder::new().skills("my skill".to_string());
        assert_eq!(builder.config.unwrap().skills_prompt, Some("my skill".to_string()));
    }

    // ── Agent ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_agent_set_and_read_skill_trust() {
        let agent = Agent::new(
            AgentConfig::default(),
            Arc::new(MockProvider),
            Arc::new(ToolRegistry::new()),
        );
        assert_eq!(agent.current_skill_trust(), crate::tools::SkillTrust::Trusted);

        agent.set_skill_trust(crate::tools::SkillTrust::Community);
        assert_eq!(agent.current_skill_trust(), crate::tools::SkillTrust::Community);

        agent.set_skill_trust(crate::tools::SkillTrust::Trusted);
        assert_eq!(agent.current_skill_trust(), crate::tools::SkillTrust::Trusted);
    }

    #[test]
    fn test_agent_update_config() {
        let mut agent = Agent::new(
            AgentConfig::default(),
            Arc::new(MockProvider),
            Arc::new(ToolRegistry::new()),
        );
        let mut new_config = AgentConfig::default();
        new_config.temperature = 0.3;
        new_config.max_tokens = 512;
        agent.update_config(new_config);
        assert_eq!(agent.config.temperature, 0.3);
        assert_eq!(agent.config.max_tokens, 512);
    }

    #[test]
    fn test_agent_with_model() {
        let agent = Agent::new(
            AgentConfig::default(),
            Arc::new(MockProvider),
            Arc::new(ToolRegistry::new()),
        )
        .with_model("claude-sonnet-4-6".to_string());
        assert_eq!(agent.model, Some("claude-sonnet-4-6".to_string()));
    }

    #[test]
    fn test_agent_builder_with_all_options() {
        let builder = AgentBuilder::new()
            .config(AgentConfig::default())
            .provider(Arc::new(MockProvider))
            .tools(Arc::new(ToolRegistry::new()));
        let result = builder.build();
        assert!(result.is_ok());
    }

    #[test]
    fn test_progress_event_clone() {
        let event = ProgressEvent::Started;
        let cloned = event.clone();
        assert!(matches!(cloned, ProgressEvent::Started));

        let event2 = ProgressEvent::ToolCalling {
            name: "search".to_string(),
            arguments: "{}".to_string(),
        };
        let cloned2 = event2.clone();
        assert!(matches!(cloned2, ProgressEvent::ToolCalling { name, .. } if name == "search"));
    }

    #[test]
    fn test_progress_event_debug() {
        let event = ProgressEvent::Completed { response: "hi".to_string() };
        let debug = format!("{:?}", event);
        assert!(debug.contains("Completed"));
    }

    #[test]
    fn test_cached_response_clone() {
        let entry = CachedResponse {
            response: "hello".to_string(),
            created_at: SystemTime::now(),
            tools_used: vec!["tool1".to_string()],
        };
        let cloned = entry.clone();
        assert_eq!(cloned.response, "hello");
        assert_eq!(cloned.tools_used, vec!["tool1"]);
    }

    #[tokio::test]
    async fn test_agent_get_chat_history_without_store() {
        let agent = Agent::new(
            AgentConfig::default(),
            Arc::new(MockProvider),
            Arc::new(ToolRegistry::new()),
        );
        let history = agent.get_chat_history("conv1", 10).await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn test_agent_get_last_conversation_without_store() {
        let agent = Agent::new(
            AgentConfig::default(),
            Arc::new(MockProvider),
            Arc::new(ToolRegistry::new()),
        );
        let last = agent.get_last_conversation("user1").await.unwrap();
        assert!(last.is_none());
    }

    // ── Workspace Dir Resolution ──────────────────────────────────────────────

    #[test]
    fn test_resolve_workspace_dir_explicit() {
        let mut config = AgentConfig::default();
        config.workspace_dir = Some(PathBuf::from("/tmp/workspace"));
        assert_eq!(config.resolve_workspace_dir(), PathBuf::from("/tmp/workspace"));
    }

    #[test]
    fn test_resolve_workspace_dir_with_tilde() {
        let mut config = AgentConfig::default();
        config.workspace_dir = Some(PathBuf::from("~/projects"));
        let resolved = config.resolve_workspace_dir();
        assert!(!resolved.to_string_lossy().contains("~"));
        assert!(resolved.to_string_lossy().contains("projects"));
    }

    #[test]
    fn test_resolve_workspace_dir_default_fallback() {
        let config = AgentConfig::default();
        let resolved = config.resolve_workspace_dir();
        assert!(resolved.to_string_lossy().contains(".manta"));
        assert!(resolved.to_string_lossy().contains("workspace"));
    }

    // ── Skill Injection ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_skill_manager_injection_into_build_fresh_context() {
        // Create skill manager and load built-in skills
        let mut skill_manager = crate::skills::SkillManager::new().await.unwrap();
        let loaded = skill_manager.load_all().await.unwrap();
        assert!(loaded > 0, "Expected built-in skills to be loaded");

        let skill_manager = Arc::new(RwLock::new(skill_manager));

        // Create agent with skill manager injected
        let agent = Agent::new(
            AgentConfig::default(),
            Arc::new(MockProvider),
            Arc::new(ToolRegistry::new()),
        )
        .with_skill_manager(skill_manager);

        // Build context with a message that should trigger the weather skill
        let ctx = agent
            .build_fresh_context("conv1", "user1", "what's the weather in Beijing")
            .await;

        let prompt = ctx.system_prompt();
        assert!(
            prompt.contains("## Active Skills"),
            "Expected '## Active Skills' section in prompt, got: {}",
            prompt
        );
        assert!(
            prompt.contains("Weather - Weather Information"),
            "Expected weather skill content in prompt, got: {}",
            prompt
        );
    }

    #[tokio::test]
    async fn test_skill_manager_no_match_without_trigger() {
        let mut skill_manager = crate::skills::SkillManager::new().await.unwrap();
        skill_manager.load_all().await.unwrap();
        let skill_manager = Arc::new(RwLock::new(skill_manager));

        let agent = Agent::new(
            AgentConfig::default(),
            Arc::new(MockProvider),
            Arc::new(ToolRegistry::new()),
        )
        .with_skill_manager(skill_manager);

        // Generic message should not trigger any skills
        let ctx = agent
            .build_fresh_context("conv2", "user1", "hello there")
            .await;

        let prompt = ctx.system_prompt();
        assert!(
            !prompt.contains("## Active Skills"),
            "Expected no skills section for generic message, got: {}",
            prompt
        );
    }
}
