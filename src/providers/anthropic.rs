//! Anthropic provider implementation for Manta
//!
//! Supports Claude 3/3.5 models with native Anthropic API format.

use super::{CompletionRequest, CompletionResponse, FunctionDefinition, Message, Provider, Role, ToolCall, ToolResult, Usage};
use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, error, instrument};

/// Anthropic API client
#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    /// API key
    api_key: String,
    /// Base URL
    base_url: String,
    /// Default model
    default_model: String,
    /// API version
    api_version: String,
    /// HTTP client
    client: reqwest::Client,
}

/// Anthropic API request body
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// Anthropic message format
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<ContentBlock>,
}

/// Content block (text or tool use)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text { text: String },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// Anthropic tool definition
#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

/// Anthropic API response
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    #[serde(rename = "type")]
    response_type: String,
    role: String,
    content: Vec<ContentBlock>,
    model: String,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

/// Anthropic usage statistics
#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

/// Anthropic error response
#[derive(Debug, Deserialize)]
struct AnthropicError {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider
    pub fn new(api_key: impl Into<String>) -> crate::Result<Self> {
        Self::with_base_url(api_key, "https://api.anthropic.com")
    }

    /// Create with custom base URL
    pub fn with_base_url(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
    ) -> crate::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| crate::error::MantaError::Internal(format!(
                "Failed to build HTTP client: {}", e
            )))?;

        Ok(Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            default_model: "claude-3-5-sonnet-20241022".to_string(),
            api_version: "2023-06-01".to_string(),
            client,
        })
    }

    /// Set the default model
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = model.into();
        self
    }

    /// Build the request URL
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    /// Build headers with authorization
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            self.api_key.parse().unwrap(),
        );
        headers.insert("anthropic-version", self.api_version.parse().unwrap());
        headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
        headers
    }

    /// Convert internal messages to Anthropic format
    fn to_anthropic_messages(messages: &[Message]) -> (Option<String>, Vec<AnthropicMessage>) {
        let mut system_prompt: Option<String> = None;
        let mut anthropic_messages = Vec::new();

        for msg in messages {
            match msg.role {
                Role::System => {
                    // System messages go in the system field, not messages array
                    system_prompt = Some(msg.content.clone());
                }
                Role::User => {
                    anthropic_messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: vec![ContentBlock::Text { text: msg.content.clone() }],
                    });
                }
                Role::Assistant => {
                    let mut content_blocks = vec![ContentBlock::Text { text: msg.content.clone() }];

                    // Add tool calls if present
                    if let Some(tool_calls) = &msg.tool_calls {
                        for tc in tool_calls {
                            content_blocks.push(ContentBlock::ToolUse {
                                id: tc.id.clone(),
                                name: tc.function.name.clone(),
                                input: serde_json::from_str(&tc.function.arguments).unwrap_or_default(),
                            });
                        }
                    }

                    anthropic_messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: content_blocks,
                    });
                }
                Role::Tool => {
                    // Tool results are separate messages in Anthropic
                    anthropic_messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: msg.tool_call_id.clone().unwrap_or_default(),
                            content: msg.content.clone(),
                            is_error: None,
                        }],
                    });
                }
            }
        }

        (system_prompt, anthropic_messages)
    }

    /// Convert Anthropic response to internal format
    fn from_anthropic_response(response: AnthropicResponse) -> CompletionResponse {
        let mut text_content = String::new();
        let mut tool_calls = Vec::new();

        for block in &response.content {
            match block {
                ContentBlock::Text { text } => {
                    text_content.push_str(text);
                }
                ContentBlock::ToolUse { id, name, input } => {
                    tool_calls.push(ToolCall {
                        id: id.clone(),
                        call_type: "tool_use".to_string(),
                        function: super::FunctionCall {
                            name: name.clone(),
                            arguments: input.to_string(),
                        },
                    });
                }
                _ => {}
            }
        }

        CompletionResponse {
            message: Message {
                role: Role::Assistant,
                content: text_content,
                name: None,
                tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
                tool_call_id: None,
                metadata: None,
            },
            usage: Some(Usage {
                prompt_tokens: response.usage.input_tokens,
                completion_tokens: response.usage.output_tokens,
                total_tokens: response.usage.input_tokens + response.usage.output_tokens,
            }),
            model: response.model,
            finish_reason: response.stop_reason,
        }
    }

    /// Convert FunctionDefinition to Anthropic tool
    fn to_anthropic_tool(func: &FunctionDefinition) -> AnthropicTool {
        AnthropicTool {
            name: func.name.clone(),
            description: func.description.clone(),
            input_schema: func.parameters.clone(),
        }
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    #[instrument(skip(self, request))]
    async fn complete(&self, request: CompletionRequest) -> crate::Result<CompletionResponse> {
        let (system, messages) = Self::to_anthropic_messages(&request.messages);

        let tools = request.tools.as_ref().map(|tools| {
            tools.iter()
                .map(|t| Self::to_anthropic_tool(&t.function))
                .collect::<Vec<_>>()
        });

        let anthropic_request = AnthropicRequest {
            model: request.model.unwrap_or_else(|| self.default_model.clone()),
            max_tokens: request.max_tokens.unwrap_or(4096),
            system,
            messages,
            tools,
            temperature: request.temperature,
            stream: Some(request.stream),
        };

        debug!("Sending request to Anthropic API");

        let response = self.client
            .post(self.url("/v1/messages"))
            .headers(self.headers())
            .json(&anthropic_request)
            .send()
            .await
            .map_err(|e| crate::error::MantaError::Http(e))?;

        let status = response.status();
        let body = response.text().await.map_err(|e| crate::error::MantaError::Http(e))?;

        if !status.is_success() {
            error!("Anthropic API error: {} - {}", status, body);
            let error_msg = format!("Anthropic API error {}: {}", status, body);
            return Err(crate::error::MantaError::ExternalService {
                source: error_msg,
                cause: None,
            });
        }

        debug!("Received response from Anthropic API");

        let anthropic_response: AnthropicResponse = serde_json::from_str(&body)
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: format!("Failed to parse Anthropic response: {}", e),
                cause: Some(Box::new(e)),
            })?;

        Ok(Self::from_anthropic_response(anthropic_response))
    }

    async fn stream(&self, _request: CompletionRequest) -> crate::Result<super::CompletionStream> {
        // Streaming not implemented yet for Anthropic
        Err(crate::error::MantaError::Internal(
            "Streaming not yet implemented for Anthropic provider".to_string()
        ))
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn max_context(&self) -> usize {
        200000 // Claude 3.5 Sonnet context window
    }

    async fn health_check(&self) -> crate::Result<bool> {
        // Simple health check by making a minimal request
        let request = CompletionRequest {
            messages: vec![Message::user("Hi")],
            model: Some(self.default_model.clone()),
            max_tokens: Some(1),
            temperature: None,
            stream: false,
            tools: None,
            stop: None,
        };

        match self.complete(request).await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_provider_creation() {
        let provider = AnthropicProvider::new("test-key").unwrap();
        assert_eq!(provider.name(), "anthropic");
        assert!(provider.supports_tools());
    }

    #[test]
    fn test_to_anthropic_messages() {
        let messages = vec![
            Message::system("You are helpful"),
            Message::user("Hello"),
            Message::assistant("Hi there!"),
        ];

        let (system, anthropic_msgs) = AnthropicProvider::to_anthropic_messages(&messages);

        assert_eq!(system, Some("You are helpful".to_string()));
        assert_eq!(anthropic_msgs.len(), 2);
        assert_eq!(anthropic_msgs[0].role, "user");
        assert_eq!(anthropic_msgs[1].role, "assistant");
    }

    #[test]
    fn test_from_anthropic_response() {
        let response = AnthropicResponse {
            id: "test-id".to_string(),
            response_type: "message".to_string(),
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text { text: "Hello!".to_string() }],
            model: "claude-3-5-sonnet".to_string(),
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
        };

        let completion = AnthropicProvider::from_anthropic_response(response);
        assert_eq!(completion.message.content, "Hello!");
        assert_eq!(completion.usage.total_tokens, 15);
    }
}
