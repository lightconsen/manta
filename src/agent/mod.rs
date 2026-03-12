//! Core Agent module for Manta
//!
//! The Agent is the central orchestrator that handles conversations,
//! manages context, calls tools, and interacts with LLM providers.

use crate::channels::{IncomingMessage, OutgoingMessage};
use crate::providers::{CompletionRequest, Message, Provider, Role, ToolCall, ToolResult};
use crate::tools::{ToolContext, ToolRegistry};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, instrument, warn};

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
        }
    }

    /// Get or create a context for a conversation
    pub async fn get_context(&self, conversation_id: &str) -> Context {
        let mut contexts = self.contexts.write().await;
        contexts
            .entry(conversation_id.to_string())
            .or_insert_with(|| {
                Context::new(
                    conversation_id.to_string(),
                    self.config.full_system_prompt(),
                    self.config.max_context_tokens,
                )
            })
            .clone()
    }

    /// Process an incoming message
    #[instrument(skip(self, message))]
    pub async fn process_message(
        &self,
        message: IncomingMessage,
    ) -> crate::Result<OutgoingMessage> {
        debug!("Processing message from user: {}", message.user_id);

        // Get or create context
        let mut context = self.get_context(&message.conversation_id.0).await;

        // Add user message to context
        context.add_message(Message::user(&message.content));

        // Get response from LLM
        let response = self.get_completion(&mut context).await?;

        // Create outgoing message
        let outgoing = OutgoingMessage::new(
            message.conversation_id,
            response.message.content.clone(),
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

    /// Build the agent
    pub fn build(self) -> crate::Result<Agent> {
        Ok(Agent::new(
            self.config.unwrap_or_default(),
            self.provider
                .ok_or_else(|| crate::error::MantaError::Validation("Provider required".to_string()))?,
            self.tools.unwrap_or_else(|| Arc::new(ToolRegistry::new())),
        ))
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
