//! LLM Provider abstractions for Manta
//!
//! This module defines the `Provider` trait for interacting with various LLM
//! services (OpenAI, Anthropic, Local models, etc.).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A message role in a conversation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System instructions to the model
    System,
    /// User input
    User,
    /// Assistant response
    Assistant,
    /// Tool output
    Tool,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => write!(f, "system"),
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

/// A single message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// The role of the message sender
    pub role: Role,
    /// The content of the message
    pub content: String,
    /// Optional name (for tool calls or multi-user scenarios)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional tool calls (for assistant messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Optional tool call ID (for tool messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
}

impl Message {
    /// Create a new system message
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            metadata: None,
        }
    }

    /// Create a new user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            metadata: None,
        }
    }

    /// Create a new assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            metadata: None,
        }
    }

    /// Create a new tool message
    pub fn tool(content: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            name: None,
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            metadata: None,
        }
    }

    /// Add a name to the message
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Add tool calls to the message
    pub fn with_tool_calls(mut self, calls: Vec<ToolCall>) -> Self {
        self.tool_calls = Some(calls);
        self
    }

    /// Add metadata to the message
    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        if self.metadata.is_none() {
            self.metadata = Some(HashMap::new());
        }
        self.metadata
            .as_mut()
            .unwrap()
            .insert(key.into(), value.into());
        self
    }
}

/// A tool call from the assistant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this tool call
    pub id: String,
    /// The type of tool call (typically "function")
    pub call_type: String,
    /// The function to call
    pub function: FunctionCall,
}

/// A function call within a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// The name of the function
    pub name: String,
    /// The arguments as a JSON string
    pub arguments: String,
}

/// The result of a tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The ID of the tool call this is a result for
    pub tool_call_id: String,
    /// The role of the result (typically "tool")
    pub role: Role,
    /// The content (result) of the tool execution
    pub content: String,
    /// Whether the tool execution was successful
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

impl ToolResult {
    /// Create a successful tool result
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            role: Role::Tool,
            content: content.into(),
            is_error: Some(false),
        }
    }

    /// Create an error tool result
    pub fn error(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            role: Role::Tool,
            content: content.into(),
            is_error: Some(true),
        }
    }
}

/// A chunk of a streaming response
#[derive(Debug, Clone)]
pub struct CompletionChunk {
    /// The content delta for this chunk
    pub content: Option<String>,
    /// Tool calls being streamed
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Whether this is the final chunk
    pub is_done: bool,
    /// Usage statistics (only in final chunk)
    pub usage: Option<Usage>,
}

/// Usage statistics for a completion
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Number of tokens in the prompt
    pub prompt_tokens: u32,
    /// Number of tokens in the completion
    pub completion_tokens: u32,
    /// Total number of tokens
    pub total_tokens: u32,
}

/// A request for text completion
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    /// The conversation history
    pub messages: Vec<Message>,
    /// Available tools
    pub tools: Option<Vec<ToolDefinition>>,
    /// Model parameters
    pub temperature: Option<f32>,
    /// Maximum tokens to generate
    pub max_tokens: Option<u32>,
    /// Whether to stream the response
    pub stream: bool,
    /// The specific model to use
    pub model: Option<String>,
    /// Stop sequences
    pub stop: Option<Vec<String>>,
}

impl Default for CompletionRequest {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            tools: None,
            temperature: Some(0.7),
            max_tokens: Some(2048),
            stream: false,
            model: None,
            stop: None,
        }
    }
}

/// A response from a completion request
#[derive(Debug, Clone)]
pub struct CompletionResponse {
    /// The message generated by the model
    pub message: Message,
    /// Usage statistics
    pub usage: Option<Usage>,
    /// The model used
    pub model: String,
    /// Finish reason
    pub finish_reason: Option<String>,
}

/// Definition of a tool for the model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The type of tool (typically "function")
    #[serde(rename = "type")]
    pub tool_type: String,
    /// The function definition
    pub function: FunctionDefinition,
}

/// Definition of a function tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// The name of the function
    pub name: String,
    /// A description of what the function does
    pub description: String,
    /// The parameters schema (JSON Schema)
    pub parameters: serde_json::Value,
}

/// A stream of completion chunks
pub type CompletionStream = std::pin::Pin<
    Box<dyn tokio_stream::Stream<Item = crate::Result<CompletionChunk>> + Send>,
>;

/// Trait for LLM providers
#[async_trait]
pub trait Provider: Send + Sync {
    /// Get the name of this provider
    fn name(&self) -> &str;

    /// Get the default model for this provider
    fn default_model(&self) -> &str;

    /// Check if this provider supports tool calling
    fn supports_tools(&self) -> bool;

    /// Get the maximum context size for this provider
    fn max_context(&self) -> usize;

    /// Complete a request (non-streaming)
    async fn complete(&self, request: CompletionRequest) -> crate::Result<CompletionResponse>;

    /// Stream a completion
    async fn stream(&self, request: CompletionRequest) -> crate::Result<CompletionStream>;

    /// Count tokens in messages (approximate if not provided by API)
    fn count_tokens(&self, messages: &[Message]) -> usize {
        // Simple approximation: 4 chars per token on average
        messages
            .iter()
            .map(|m| m.content.len() / 4)
            .sum()
    }

    /// Check if the provider is healthy
    async fn health_check(&self) -> crate::Result<bool>;
}

/// Registry of providers
#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn Provider>>,
}

impl std::fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRegistry")
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ProviderRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider
    pub fn register(&mut self, provider: Box<dyn Provider>) {
        let name = provider.name().to_string();
        self.providers.insert(name, provider);
    }

    /// Get a provider by name
    pub fn get(&self, name: &str) -> Option<&dyn Provider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    /// List available provider names
    pub fn list(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a provider exists
    pub fn has(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }
}

pub mod anthropic;
pub mod openai;

pub use anthropic::AnthropicProvider;
pub use openai::OpenAiProvider;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let system = Message::system("You are a helpful assistant");
        assert_eq!(system.role, Role::System);
        assert_eq!(system.content, "You are a helpful assistant");

        let user = Message::user("Hello!");
        assert_eq!(user.role, Role::User);
        assert_eq!(user.content, "Hello!");

        let assistant = Message::assistant("Hi there!");
        assert_eq!(assistant.role, Role::Assistant);
        assert_eq!(assistant.content, "Hi there!");
    }

    #[test]
    fn test_tool_result() {
        let success = ToolResult::success("call_123", "Result data");
        assert_eq!(success.tool_call_id, "call_123");
        assert_eq!(success.is_error, Some(false));

        let error = ToolResult::error("call_456", "Something went wrong");
        assert_eq!(error.is_error, Some(true));
    }

    #[test]
    fn test_provider_registry() {
        let mut registry = ProviderRegistry::new();
        assert!(registry.list().is_empty());

        // We can't easily test with real providers, but we can test the interface
        assert!(!registry.has("test"));
        assert!(registry.get("test").is_none());
    }
}
