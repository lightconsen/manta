//! Core Agent module for Manta
//!
//! The Agent is the central orchestrator that handles conversations,
//! manages context, calls tools, and interacts with LLM providers.

use crate::channels::{IncomingMessage, OutgoingMessage};
use crate::memory::MemoryStore;
use crate::providers::{CompletionRequest, Message, Provider, Role, ToolCall, ToolResult};
use crate::tools::{ToolContext, ToolRegistry};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, instrument};

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
pub type ProgressCallback = Arc<dyn Fn(ProgressEvent) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

pub mod budget;
pub mod compressor;
pub mod context;
pub mod todo;

pub use budget::{BudgetConfig, BudgetExhaustionAction, IterationBudget};
pub use compressor::{CompressionStats, CompressionStrategy, ContextCompressor};
pub use context::Context;
pub use todo::{Task, TaskStatus, TodoStore};

/// Configuration for the Agent
#[derive(Debug, Clone)]
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
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: "You are Manta, a helpful AI assistant.".to_string(),
            max_context_tokens: 4096,
            max_concurrent_tools: 5,
            temperature: 0.7,
            max_tokens: 2048,
            skills_prompt: None,
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
}

impl Agent {
    /// Create a new Agent
    pub fn new(
        config: AgentConfig,
        provider: Arc<dyn Provider>,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            config,
            provider,
            tools,
            contexts: Arc::new(RwLock::new(std::collections::HashMap::new())),
            shutdown_tx: Arc::new(RwLock::new(None)),
            memory_store: None,
            chat_history: None,
            session_search: None,
        }
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

    /// Get or create a context for a conversation
    pub async fn get_context(&self, conversation_id: &str, user_id: &str) -> Context {
        let mut contexts = self.contexts.write().await;

        // Check if context already exists
        if let Some(context) = contexts.get(conversation_id) {
            return context.clone();
        }

        // Create new context with personality-loaded system prompt
        let mut system_prompt = self
            .config
            .full_system_prompt_with_personality()
            .await;

        // Inject relevant memories if memory store is available
        if let Some(ref store) = self.memory_store {
            // Get recent memories for this user
            let memory_query = crate::memory::MemoryQuery::new()
                .for_user(user_id)
                .limit(5);

            match store.search(memory_query).await {
                Ok(memories) if !memories.is_empty() => {
                    let memory_section = crate::tools::MemoryTool::format_memories_for_prompt(&memories);
                    if !memory_section.is_empty() {
                        system_prompt.push_str("\n\n");
                        system_prompt.push_str(&memory_section);
                    }
                }
                _ => {}
            }
        }

        let context = Context::new(
            conversation_id.to_string(),
            system_prompt,
            self.config.max_context_tokens,
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

        // Store user message in chat history and index for search
        let message_id = uuid::Uuid::new_v4().to_string();
        if let Some(ref store) = self.chat_history {
            use crate::memory::{ChatHistoryStore, ChatMessage};
            let chat_msg = ChatMessage::new(
                &conversation_id,
                &user_id,
                "user",
                &content,
            );
            // Clone message_id before moving chat_msg
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store user message: {}", e);
            }
            // Index for session search
            if let Some(ref search) = self.session_search {
                if let Err(e) = search.index_message(&msg_id, &conversation_id, &user_id, &content, "user").await {
                    error!("Failed to index user message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            // Even if chat history is not enabled, index for search
            if let Err(e) = search.index_message(&message_id, &conversation_id, &user_id, &content, "user").await {
                error!("Failed to index user message for search: {}", e);
            }
        }

        // Get or create context
        let mut context = self.get_context(&conversation_id, &user_id).await;

        // Add user message to context
        context.add_message(Message::user(&content));

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
                if let Err(e) = search.index_message(&msg_id, &conversation_id, &user_id, &response.message.content, "assistant").await {
                    error!("Failed to index assistant message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            // Even if chat history is not enabled, index for search
            if let Err(e) = search.index_message(&assistant_message_id, &conversation_id, &user_id, &response.message.content, "assistant").await {
                error!("Failed to index assistant message for search: {}", e);
            }
        }

        // Create outgoing message
        let outgoing = OutgoingMessage::new(
            crate::channels::ConversationId(conversation_id),
            response.message.content.clone(),
        );

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

        // Store user message in chat history and index for search
        let message_id = uuid::Uuid::new_v4().to_string();
        if let Some(ref store) = self.chat_history {
            use crate::memory::{ChatHistoryStore, ChatMessage};
            let chat_msg = ChatMessage::new(
                &conversation_id,
                &user_id,
                "user",
                &content,
            );
            let msg_id = chat_msg.id.clone();
            if let Err(e) = store.store_message(chat_msg).await {
                error!("Failed to store user message: {}", e);
            }
            if let Some(ref search) = self.session_search {
                if let Err(e) = search.index_message(&msg_id, &conversation_id, &user_id, &content, "user").await {
                    error!("Failed to index user message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            if let Err(e) = search.index_message(&message_id, &conversation_id, &user_id, &content, "user").await {
                error!("Failed to index user message for search: {}", e);
            }
        }

        // Get or create context
        let mut context = self.get_context(&conversation_id, &user_id).await;

        // Add user message to context
        context.add_message(Message::user(&content));

        // Get response from LLM with progress
        let response = self.get_completion_with_progress(&mut context, progress_cb.clone()).await?;

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
                if let Err(e) = search.index_message(&msg_id, &conversation_id, &user_id, &response.message.content, "assistant").await {
                    error!("Failed to index assistant message for search: {}", e);
                }
            }
        } else if let Some(ref search) = self.session_search {
            if let Err(e) = search.index_message(&assistant_message_id, &conversation_id, &user_id, &response.message.content, "assistant").await {
                error!("Failed to index assistant message for search: {}", e);
            }
        }

        // Notify completed
        let response_content = response.message.content.clone();
        (progress_cb)(ProgressEvent::Completed { response: response_content.clone() }).await;

        // Create outgoing message
        let outgoing = OutgoingMessage::new(
            crate::channels::ConversationId(conversation_id),
            response_content,
        );

        Ok(outgoing)
    }

    /// Get a completion from the LLM, handling tool calls
    async fn get_completion(&self, context: &mut Context) -> crate::Result<crate::providers::CompletionResponse> {
        let messages = context.to_messages();

        // Get available tools
        let tool_context = ToolContext::new("user", context.id());
        let tool_defs = self.tools.get_available(&tool_context);
        let has_tools = !tool_defs.is_empty();

        let mut request = CompletionRequest {
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

        // Get completion
        let response = self.provider.complete(request).await?;

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
        // Add assistant message with tool calls
        context.add_message(original_response.message.clone());

        // Execute tools concurrently (up to limit)
        let tool_context = ToolContext::new("user", context.id())
            .with_timeout(std::time::Duration::from_secs(30));

        let mut results = Vec::new();

        for tool_call in tool_calls.iter().take(self.config.max_concurrent_tools) {
            debug!("Executing tool: {}", tool_call.function.name);

            let result = match self.tools.execute_call(&tool_call.function, &tool_context).await {
                Ok(exec_result) => {
                    let tool_result = exec_result.to_tool_result(&tool_call.id);
                    info!("Tool {} executed successfully", tool_call.function.name);
                    tool_result
                }
                Err(e) => {
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
        // Add assistant message with tool calls
        context.add_message(original_response.message.clone());

        // Execute tools with progress
        let tool_context = ToolContext::new("user", context.id())
            .with_timeout(std::time::Duration::from_secs(30));

        let mut results = Vec::new();

        for tool_call in tool_calls.iter().take(self.config.max_concurrent_tools) {
            let tool_name = tool_call.function.name.clone();
            let tool_args = tool_call.function.arguments.clone();

            // Notify tool calling
            (progress_cb)(ProgressEvent::ToolCalling {
                name: tool_name.clone(),
                arguments: tool_args,
            })
            .await;

            debug!("Executing tool: {}", tool_name);

            let result = match self.tools.execute_call(&tool_call.function, &tool_context).await {
                Ok(exec_result) => {
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
            self.provider
                .ok_or_else(|| crate::error::MantaError::Validation("Provider required".to_string()))?,
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
