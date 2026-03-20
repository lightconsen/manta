//! Core Agent module for Manta
//!
//! The Agent is the central orchestrator that handles conversations,
//! manages context, calls tools, and interacts with LLM providers.

use crate::channels::{IncomingMessage, OutgoingMessage};
use crate::memory::MemoryStore;
use crate::providers::{CompletionRequest, Message, Provider, Role, ToolCall, ToolResult};
use crate::tools::{ToolContext, ToolRegistry};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, RwLock};
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
    /// LLM is generating response
    Generating,
    /// Completed with final response
    Completed { response: String },
    /// Error occurred
    Error { message: String },
}

/// Callback type for progress updates
pub type ProgressCallback = Arc<
    dyn Fn(ProgressEvent) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync,
>;

pub mod budget;
pub mod compressor;
pub mod context;
pub mod cost_guard;
pub mod personality;
pub mod planner;
pub mod prompt_builder;
pub mod session;
pub mod session_store;
pub mod subagent_registry;
pub mod todo;
pub mod turns;

pub use budget::{BudgetConfig, BudgetExhaustionAction, IterationBudget};
pub use compressor::{CompressionStats, CompressionStrategy, ContextCompressor};
pub use context::Context;
pub use cost_guard::CostGuard;
pub use personality::{AgentPersonality, AgentRegistry, PersonalityContext, SharedAgentRegistry};
pub use planner::{ActivePlan, TaskPlan, TaskPlanner};
pub use prompt_builder::{ConversationPhase, PromptBuilder, PromptContext, TaskType};
pub use session::{
    AgentInstanceStatus, MultiAgentSession, SessionAgent, SessionManager, SessionMessage,
    SessionStatus, ThreadBinding,
};
pub use subagent_registry::{SubagentMetrics, SubagentRegistry, SubagentRun, SubagentStatus};
pub use todo::{Task, TaskStatus, TodoStore};
pub use turns::{Thread, ThreadManager, Turn, TurnState};

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
        message.replace('"', "\"")
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

    /// Get the full system prompt including personality memory and skills
    pub async fn full_system_prompt_with_personality(&self) -> String {
        let base_prompt = self.full_system_prompt();

        // Load personality memory
        match crate::memory::PersonalityMemory::new().await {
            Ok(memory) => {
                // Initialize default files if they don't exist
                let _ = memory.initialize_defaults().await;

                let personality = memory.format_for_prompt().await.unwrap_or_default();
                if personality.is_empty() {
                    base_prompt
                } else {
                    format!("{}\n{}", base_prompt, personality)
                }
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
    /// Context storage
    contexts: Arc<RwLock<std::collections::HashMap<String, Context>>>,
    /// Shutdown signal
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    /// Memory store for persistence
    memory_store: Option<Arc<crate::memory::SqliteMemoryStore>>,
    /// Chat history store for conversation persistence
    chat_history: Option<Arc<crate::memory::SqliteMemoryStore>>,
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
            contexts: Arc::new(RwLock::new(std::collections::HashMap::new())),
            shutdown_tx: Arc::new(RwLock::new(None)),
            memory_store: None,
            chat_history: None,
            session_search: None,
            response_cache: Arc::new(ResponseCache::new(Duration::from_secs(3600))), // 1 hour TTL
            task_planner: Arc::new(TaskPlanner::new(provider_clone)),
            active_plans: Arc::new(RwLock::new(std::collections::HashMap::new())),
            cost_guard: None,
        }
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
    pub fn with_memory_store(mut self, store: Arc<crate::memory::SqliteMemoryStore>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Set the chat history store
    pub fn with_chat_history(mut self, store: Arc<crate::memory::SqliteMemoryStore>) -> Self {
        self.chat_history = Some(store);
        self
    }

    /// Set the session search for conversation indexing
    pub fn with_session_search(mut self, search: Arc<crate::memory::SessionSearch>) -> Self {
        self.session_search = Some(search);
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
            use crate::memory::ChatHistoryStore;
            store.get_conversation_history(conversation_id, limit).await
        } else {
            Ok(Vec::new())
        }
    }

    /// Get the last conversation ID for a user
    pub async fn get_last_conversation(&self, user_id: &str) -> crate::Result<Option<String>> {
        if let Some(ref store) = self.chat_history {
            use crate::memory::ChatHistoryStore;
            store.get_last_conversation(user_id).await
        } else {
            Ok(None)
        }
    }

    /// Get or create a context for a conversation with dynamic prompt building
    pub async fn get_context(
        &self,
        conversation_id: &str,
        user_id: &str,
        user_message: &str,
    ) -> Context {
        let mut contexts = self.contexts.write().await;

        // Check if context already exists - if so, we may update it dynamically
        let existing_context = contexts.get(conversation_id).cloned();

        // Build dynamic prompt context
        let mut prompt_ctx = PromptContext::new(user_message);
        prompt_ctx.detect_task_type();

        // Set phase based on existing context or new conversation
        let history_len = existing_context
            .as_ref()
            .map(|c| c.history().len())
            .unwrap_or(0);
        prompt_ctx = prompt_ctx.set_phase(history_len);

        // Check for active plan
        let active_plans = self.active_plans.read().await;
        if let Some(active_plan) = active_plans.get(conversation_id) {
            if let Some(task_prompt) = active_plan.current_task_prompt() {
                prompt_ctx.task_context = Some(task_prompt);
            }
        }
        drop(active_plans);

        // Get available tools
        let tool_context = crate::tools::ToolContext::new(user_id, conversation_id);
        let tool_defs = self.tools.get_available(&tool_context);
        prompt_ctx.available_tools = tool_defs;

        // Get base prompt
        let base_prompt = self.config.full_system_prompt_with_personality().await;

        // Build dynamic system prompt
        let system_prompt = PromptBuilder::build_from_context(
            &base_prompt,
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

        contexts.insert(conversation_id.to_string(), context.clone());
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
                    use crate::memory::{ChatHistoryStore, ChatMessage};
                    let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store user message: {}", e);
                    }
                }

                // Store cached assistant response in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::{ChatHistoryStore, ChatMessage};
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
        if let Some(ref store) = self.chat_history {
            use crate::memory::{ChatHistoryStore, ChatMessage};
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

        // Get or create context with dynamic prompt building
        let mut context = self.get_context(&conversation_id, &user_id, &content).await;
        // Reset tool tracking for this turn
        context.clear_tools_used();

        // Add user message to context
        context.add_message(Message::user(&content));

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

        // Get response from LLM
        let response = self.get_completion(&mut context).await?;

        // Store assistant response in chat history and index for search
        let assistant_message_id = uuid::Uuid::new_v4().to_string();
        if let Some(ref store) = self.chat_history {
            use crate::memory::{ChatHistoryStore, ChatMessage};
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
            let tools_used = context.tools_used().to_vec();
            // Check if tools used are cacheable (skip cache for time-sensitive tools)
            if are_tools_cacheable(&tools_used) {
                self.response_cache
                    .set(
                        &user_id,
                        &conversation_id,
                        &content,
                        response.message.content.clone(),
                        tools_used,
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
            outgoing.usage = Some(usage.clone());
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
                    use crate::memory::{ChatHistoryStore, ChatMessage};
                    let chat_msg = ChatMessage::new(&conversation_id, &user_id, "user", &content);
                    if let Err(e) = store.store_message(chat_msg).await {
                        error!("Failed to store user message: {}", e);
                    }
                }

                // Store cached assistant response in chat history
                if let Some(ref store) = self.chat_history {
                    use crate::memory::{ChatHistoryStore, ChatMessage};
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
            use crate::memory::{ChatHistoryStore, ChatMessage};
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

        // Get or create context with dynamic prompt building
        let mut context = self.get_context(&conversation_id, &user_id, &content).await;
        // Reset tool tracking for this turn
        context.clear_tools_used();

        // Add user message to context
        context.add_message(Message::user(&content));

        // Get response from LLM with progress
        let response = self
            .get_completion_with_progress(&mut context, progress_cb.clone())
            .await?;

        // Store assistant response
        let assistant_message_id = uuid::Uuid::new_v4().to_string();
        if let Some(ref store) = self.chat_history {
            use crate::memory::{ChatHistoryStore, ChatMessage};
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
        if should_cache {
            let tools_used = context.tools_used().to_vec();
            if are_tools_cacheable(&tools_used) {
                self.response_cache
                    .set(&user_id, &conversation_id, &content, response.message.content.clone(), tools_used)
                    .await;
            }
        }

        // Notify completed
        let response_content = response.message.content.clone();
        (progress_cb)(ProgressEvent::Completed {
            response: response_content.clone(),
        })
        .await;

        // Create outgoing message
        let outgoing = OutgoingMessage::new(
            crate::channels::ConversationId(conversation_id),
            response_content,
        );

        Ok(outgoing)
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
                let compressor =
                    crate::agent::compressor::ContextCompressor::new(self.config.max_context_tokens);
                let history = context.history().to_vec();
                let compacted = compressor
                    .compact_with_llm(&history, &self.provider, Some(compaction_model.as_str()), 2, 6)
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
        let tool_context = ToolContext::new("user", context.id());
        let tool_defs = self.tools.get_available(&tool_context);
        let has_tools = !tool_defs.is_empty();

        let mut request = CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            stream: false,
            ..Default::default()
        };

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

        // Get completion
        let response = self.provider.complete(request).await?;

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
        let tool_context =
            ToolContext::new("user", context.id()).with_timeout(std::time::Duration::from_secs(30));

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
                name: None,
                tool_calls: None,
                tool_call_id: Some(result.tool_call_id),
                metadata: None,
            });
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
        let tool_context = ToolContext::new("user", context.id());
        let tool_defs = self.tools.get_available(&tool_context);
        let has_tools = !tool_defs.is_empty();

        let mut request = CompletionRequest {
            model: self.model.clone(),
            messages,
            temperature: Some(self.config.temperature),
            max_tokens: Some(self.config.max_tokens),
            stream: false,
            ..Default::default()
        };

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

        // Notify generating
        (progress_cb)(ProgressEvent::Generating).await;

        // Get completion
        let response = self.provider.complete(request).await?;

        // Handle tool calls if present
        if let Some(tool_calls) = &response.message.tool_calls {
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
        let tool_context =
            ToolContext::new("user", context.id()).with_timeout(std::time::Duration::from_secs(30));

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

        // Add tool results to context
        for result in results {
            context.add_message(Message {
                role: Role::Tool,
                content: result.content,
                name: None,
                tool_calls: None,
                tool_call_id: Some(result.tool_call_id),
                metadata: None,
            });
        }

        // Get final response with progress
        Box::pin(self.get_completion_with_progress(context, progress_cb)).await
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
        let contexts = Arc::clone(&self.contexts);
        let tools = Arc::clone(&self.tools);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(check_interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                ticker.tick().await;

                // ── 1. Evict stale contexts ───────────────────────────────────
                let stale_ids: Vec<String> = {
                    let guard = contexts.read().await;
                    guard
                        .iter()
                        .filter(|(_, ctx)| ctx.is_stale(stale_threshold))
                        .map(|(id, _)| id.clone())
                        .collect()
                };

                if !stale_ids.is_empty() {
                    let mut guard = contexts.write().await;
                    for id in &stale_ids {
                        guard.remove(id);
                        warn!(
                            conversation_id = id.as_str(),
                            "Self-repair: evicted stale context (inactive >{:?})",
                            stale_threshold
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

    /// Shutdown the agent
    pub async fn shutdown(&self) -> crate::Result<()> {
        info!("Shutting down agent");
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
}

/// Builder for Agent
#[derive(Default)]
pub struct AgentBuilder {
    config: Option<AgentConfig>,
    provider: Option<Arc<dyn Provider>>,
    tools: Option<Arc<ToolRegistry>>,
    memory_store: Option<Arc<crate::memory::SqliteMemoryStore>>,
    chat_history: Option<Arc<crate::memory::SqliteMemoryStore>>,
    session_search: Option<Arc<crate::memory::SessionSearch>>,
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
    pub fn memory_store(mut self, store: Arc<crate::memory::SqliteMemoryStore>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Set chat history store for conversation persistence
    pub fn chat_history(mut self, store: Arc<crate::memory::SqliteMemoryStore>) -> Self {
        self.chat_history = Some(store);
        self
    }

    /// Set session search for conversation indexing
    pub fn session_search(mut self, search: Arc<crate::memory::SessionSearch>) -> Self {
        self.session_search = Some(search);
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

        Ok(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.max_context_tokens, 4096);
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.max_tokens, 2048);
    }

    #[test]
    fn test_agent_builder() {
        let builder = AgentBuilder::new();
        assert!(builder.config.is_none());
        assert!(builder.provider.is_none());
    }
}
