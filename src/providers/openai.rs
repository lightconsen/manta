//! OpenAI provider implementation for Manta
//!
//! Supports GPT-4, GPT-3.5, and other OpenAI models.

use super::{
    CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream, FunctionDefinition,
    Message, Provider, Role, ToolCall, ToolDefinition, Usage,
};
use async_trait::async_trait;
use futures_core::Stream;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tracing::{debug, error, instrument, warn};

/// OpenAI API client
#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    /// API key
    api_key: String,
    /// Base URL (default: https://api.openai.com/v1)
    base_url: String,
    /// Default model
    default_model: String,
    /// HTTP client
    client: reqwest::Client,
}

impl OpenAiProvider {
    /// Create a new OpenAI provider
    pub fn new(api_key: impl Into<String>) -> crate::Result<Self> {
        Self::with_base_url(api_key, "https://api.openai.com/v1")
    }

    /// Create with custom base URL (for proxies or Azure)
    pub fn with_base_url(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
    ) -> crate::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .default_headers(headers)
            .build()
            .map_err(|e| {
                crate::error::MantaError::Internal(format!("Failed to build HTTP client: {}", e))
            })?;

        Ok(Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            default_model: "gpt-4o-mini".to_string(),
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
        // Support custom paths via MANTA_API_PATH env var
        let custom_path = std::env::var("MANTA_API_PATH").ok();
        if let Some(api_path) = custom_path {
            format!("{}/{}", self.base_url.trim_end_matches('/'), api_path.trim_start_matches('/'))
        } else {
            format!("{}{}", self.base_url.trim_end_matches('/'), path)
        }
    }

    /// Build headers with authorization
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", self.api_key).parse().unwrap(),
        );
        headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
        headers
    }

    /// Convert internal message to OpenAI format
    fn to_openai_message(msg: &Message) -> OpenAiMessage {
        OpenAiMessage {
            role: match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            }
            .to_string(),
            content: Some(msg.content.clone()),
            name: msg.name.clone(),
            tool_calls: msg.tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|tc| OpenAiToolCall {
                        id: tc.id.clone(),
                        call_type: tc.call_type.clone(),
                        function: OpenAiFunctionCall {
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        },
                    })
                    .collect()
            }),
            tool_call_id: msg.tool_call_id.clone(),
        }
    }

    /// Convert OpenAI response to internal format
    fn from_openai_response(&self, resp: OpenAiResponse) -> crate::Result<CompletionResponse> {
        let choice = resp.choices.into_iter().next().ok_or_else(|| {
            crate::error::MantaError::ExternalService {
                source: "No completion choices returned".to_string(),
                cause: None,
            }
        })?;

        let message = Message {
            role: match choice.message.role.as_str() {
                "system" => Role::System,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            },
            content: choice.message.content.unwrap_or_default(),
            name: choice.message.name,
            tool_calls: choice.message.tool_calls.map(|calls| {
                calls
                    .into_iter()
                    .map(|tc| ToolCall {
                        id: tc.id,
                        call_type: tc.call_type,
                        function: super::FunctionCall {
                            name: tc.function.name,
                            arguments: tc.function.arguments,
                        },
                    })
                    .collect()
            }),
            tool_call_id: choice.message.tool_call_id,
            metadata: None,
        };

        Ok(CompletionResponse {
            message,
            usage: resp.usage.map(|u| Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            }),
            model: resp.model,
            finish_reason: choice.finish_reason,
        })
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn max_context(&self) -> usize {
        match self.default_model.as_str() {
            "gpt-4o" | "gpt-4-turbo" => 128_000,
            "gpt-4" => 8_192,
            "gpt-3.5-turbo" => 16_385,
            _ => 4_096,
        }
    }

    #[instrument(skip(self, request))]
    async fn complete(&self, request: CompletionRequest) -> crate::Result<CompletionResponse> {
        debug!("Sending completion request to OpenAI");

        let model = request.model.unwrap_or_else(|| self.default_model.clone());

        let tools: Option<Vec<OpenAiTool>> = request.tools.map(|tools| {
            tools
                .into_iter()
                .map(|t| OpenAiTool {
                    tool_type: "function".to_string(),
                    function: t.function,
                })
                .collect()
        });

        let body = OpenAiRequest {
            model,
            messages: request.messages.iter().map(Self::to_openai_message).collect(),
            tools,
            temperature: request.temperature.unwrap_or(0.7),
            max_tokens: request.max_tokens,
            stream: Some(false),
            stop: request.stop,
        };

        let response = self
            .client
            .post(self.url("/chat/completions"))
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::MantaError::Http(e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!("OpenAI API error: {} - {}", status, text);
            return Err(crate::error::MantaError::ExternalService {
                source: format!("OpenAI API error {}: {}", status, text),
                cause: None,
            });
        }

        let openai_resp: OpenAiResponse = response.json().await.map_err(|e| {
            crate::error::MantaError::ExternalService {
                source: format!("Failed to parse OpenAI response: {}", e),
                cause: Some(Box::new(e)),
            }
        })?;

        debug!("Received completion from OpenAI");
        self.from_openai_response(openai_resp)
    }

    async fn stream(&self, request: CompletionRequest) -> crate::Result<CompletionStream> {
        debug!("Starting streaming completion from OpenAI");

        let model = request.model.unwrap_or_else(|| self.default_model.clone());

        let tools: Option<Vec<OpenAiTool>> = request.tools.map(|tools| {
            tools
                .into_iter()
                .map(|t| OpenAiTool {
                    tool_type: "function".to_string(),
                    function: t.function,
                })
                .collect()
        });

        let body = OpenAiRequest {
            model,
            messages: request.messages.iter().map(Self::to_openai_message).collect(),
            tools,
            temperature: request.temperature.unwrap_or(0.7),
            max_tokens: request.max_tokens,
            stream: Some(true),
            stop: request.stop,
        };

        let response = self
            .client
            .post(self.url("/chat/completions"))
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .map_err(crate::error::MantaError::Http)?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            error!("OpenAI API error: {} - {}", status, text);
            return Err(crate::error::MantaError::ExternalService {
                source: format!("OpenAI API error {}: {}", status, text),
                cause: None,
            });
        }

        let stream = response.bytes_stream();
        let openai_stream = OpenAiStream::new(stream);

        Ok(Box::pin(openai_stream))
    }

    async fn health_check(&self) -> crate::Result<bool> {
        // Simple check by listing models
        let response = self
            .client
            .get(self.url("/models"))
            .headers(self.headers())
            .send()
            .await
            .map_err(|e| crate::error::MantaError::Http(e))?;

        Ok(response.status().is_success())
    }
}

// OpenAI API types

#[derive(Debug, Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: FunctionDefinition,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAiFunctionCall,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenAiFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    index: u32,
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

// SSE Streaming types

#[derive(Debug, Deserialize)]
struct OpenAiStreamResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAiStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    index: u32,
    delta: OpenAiDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAiDelta {
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Debug, Deserialize, Clone)]
struct OpenAiStreamToolCall {
    index: u32,
    id: Option<String>,
    #[serde(rename = "type")]
    call_type: Option<String>,
    function: Option<OpenAiStreamFunctionCall>,
}

#[derive(Debug, Deserialize, Clone)]
struct OpenAiStreamFunctionCall {
    name: Option<String>,
    arguments: Option<String>,
}

/// OpenAI SSE stream parser
struct OpenAiStream {
    buffer: String,
    inner: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
}

impl OpenAiStream {
    fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
    {
        Self {
            buffer: String::new(),
            inner: Box::pin(stream),
        }
    }

    fn parse_sse_line(&self, line: &str) -> Option<CompletionChunk> {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with(':') {
            return None;
        }

        // Parse data lines
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                return Some(CompletionChunk {
                    content: None,
                    tool_calls: None,
                    is_done: true,
                    usage: None,
                });
            }

            // Try to parse the JSON
            if let Ok(response) = serde_json::from_str::<OpenAiStreamResponse>(data) {
                if let Some(choice) = response.choices.first() {
                    let content = choice.delta.content.clone();

                    // Convert tool calls
                    let tool_calls = choice.delta.tool_calls.as_ref().map(|calls| {
                        calls
                            .iter()
                            .filter_map(|tc| {
                                Some(ToolCall {
                                    id: tc.id.clone()?,
                                    call_type: tc.call_type.clone()?,
                                    function: super::FunctionCall {
                                        name: tc.function.as_ref()?.name.clone()?,
                                        arguments: tc.function.as_ref()?.arguments.clone()?,
                                    },
                                })
                            })
                            .collect()
                    });

                    let is_done = choice.finish_reason.is_some();

                    return Some(CompletionChunk {
                        content,
                        tool_calls,
                        is_done,
                        usage: None, // Usage not typically sent in stream chunks
                    });
                }
            }
        }

        None
    }
}

impl Stream for OpenAiStream {
    type Item = CompletionChunk;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    // Convert bytes to string and add to buffer
                    if let Ok(chunk) = std::str::from_utf8(&bytes) {
                        self.buffer.push_str(chunk);

                        // Process complete lines from buffer
                        while let Some(pos) = self.buffer.find('\n') {
                            let line = self.buffer[..pos].to_string();
                            self.buffer = self.buffer[pos + 1..].to_string();

                            if let Some(chunk) = self.parse_sse_line(&line) {
                                return Poll::Ready(Some(chunk));
                            }
                        }
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    warn!("Stream error: {}", e);
                    return Poll::Ready(None);
                }
                Poll::Ready(None) => {
                    // Process any remaining data in buffer
                    if !self.buffer.is_empty() {
                        let line = self.buffer.clone();
                        self.buffer.clear();
                        if let Some(chunk) = self.parse_sse_line(&line) {
                            return Poll::Ready(Some(chunk));
                        }
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_message_conversion() {
        let msg = Message::user("Hello");
        let openai = OpenAiProvider::to_openai_message(&msg);
        assert_eq!(openai.role, "user");
        assert_eq!(openai.content, Some("Hello".to_string()));
    }

    #[test]
    fn test_openai_message_conversion_assistant() {
        let msg = Message::assistant("Hi there!");
        let openai = OpenAiProvider::to_openai_message(&msg);
        assert_eq!(openai.role, "assistant");
    }

    #[test]
    fn test_max_context() {
        let provider = OpenAiProvider::new("test-key").unwrap();
        assert!(provider.max_context() > 0);
    }

    #[test]
    fn test_url_building() {
        let provider = OpenAiProvider::new("test-key").unwrap();
        assert_eq!(
            provider.url("/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
    }
}
