//! Anthropic provider implementation for Syscity
//!
//! Supports Claude 3/3.5 models with native Anthropic API format.

use super::{
    stream_wrappers::ProviderStreamFamily, CompletionChunk, CompletionRequest,
    CompletionResponse, CompletionStream, FunctionDefinition, Message, Provider,
    ProviderInstanceConfig, Role, ToolCall, Usage,
};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, error, info, instrument, warn};

/// Anthropic API client
#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    /// Authentication credential (supports API key, Bearer token, OAuth2)
    credential: std::sync::Arc<tokio::sync::RwLock<crate::model_router::Credential>>,
    /// Base URL
    base_url: String,
    /// Default model
    default_model: String,
    /// API version
    api_version: String,
    /// HTTP client
    client: reqwest::Client,
    /// Optional stream family override (e.g. for Kimi Anthropic endpoint)
    stream_family_override: Option<ProviderStreamFamily>,
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

/// Content block (text, image, or tool use)
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: ImageSource,
    },
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

/// Anthropic image source format
#[derive(Debug, Serialize, Deserialize, Clone)]
struct ImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
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
#[allow(dead_code)]
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
#[allow(dead_code)]
struct AnthropicError {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

/// Anthropic streaming event
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    delta: Option<StreamDelta>,
    #[serde(default)]
    message: Option<StreamMessage>,
    #[serde(default)]
    usage: Option<StreamUsage>,
}

/// Delta in streaming response
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamDelta {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
}

/// Message start in streaming
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamMessage {
    usage: Option<StreamUsage>,
}

/// Usage in streaming
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamUsage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider from an API key string (backward-compatible).
    pub fn new(api_key: impl Into<String>) -> crate::Result<Self> {
        Self::with_credential(crate::model_router::Credential::api_key(api_key))
    }

    /// Create with custom base URL from an API key string (backward-compatible).
    pub fn with_base_url(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
    ) -> crate::Result<Self> {
        let mut this = Self::with_credential(crate::model_router::Credential::api_key(api_key))?;
        this.base_url = base_url.into();
        Ok(this)
    }

    /// Create with a full `Credential` (supports OAuth2, Bearer token, API key).
    pub fn with_credential(credential: crate::model_router::Credential) -> crate::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!("Failed to build HTTP client: {}", e))
            })?;

        Ok(Self {
            credential: std::sync::Arc::new(tokio::sync::RwLock::new(credential)),
            base_url: "https://api.anthropic.com".to_string(),
            default_model: "claude-3-5-sonnet-20241022".to_string(),
            api_version: "2023-06-01".to_string(),
            client,
            stream_family_override: None,
        })
    }

    /// Create from a fully-resolved `ProviderInstanceConfig`.
    ///
    /// This is the primary constructor used by the resolver; it sets all fields
    /// including protocol-variant-specific stream families (e.g., Kimi Anthropic).
    pub fn from_config(config: ProviderInstanceConfig) -> crate::Result<Self> {
        let credential =
            crate::model_router::Credential::api_key(config.api_key.unwrap_or_default());
        let mut this = Self::with_credential(credential)?;
        this.base_url = config.base_url;
        this.default_model = config.model;
        this.stream_family_override = Some(config.stream_family);
        Ok(this)
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

    /// Refresh the credential if it is expired or expiring soon.
    async fn refresh_auth(&self) -> crate::Result<()> {
        let mut cred = self.credential.write().await;
        cred.refresh_if_needed(&self.client).await
    }

    /// Build headers with authorization (async to read the RwLock).
    async fn headers(&self) -> HeaderMap {
        let cred = self.credential.read().await;
        let mut headers = HeaderMap::new();
        match &*cred {
            crate::model_router::Credential::ApiKey { key } => {
                headers.insert("x-api-key", key.parse().unwrap());
            }
            _ => {
                headers.insert("Authorization", cred.authorization_header().parse().unwrap());
            }
        }
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
                    let content = if let Some(ref blocks) = msg.content_blocks {
                        blocks
                            .iter()
                            .map(|b| match b {
                                super::ContentBlock::Text { text } => {
                                    ContentBlock::Text { text: text.clone() }
                                }
                                super::ContentBlock::Image { base64, mime_type } => {
                                    ContentBlock::Image {
                                        source: ImageSource {
                                            source_type: "base64".to_string(),
                                            media_type: mime_type.clone(),
                                            data: base64.clone(),
                                        },
                                    }
                                }
                            })
                            .collect()
                    } else {
                        vec![ContentBlock::Text { text: msg.content.clone() }]
                    };
                    anthropic_messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content,
                    });
                }
                Role::Assistant => {
                    let mut content = if let Some(ref blocks) = msg.content_blocks {
                        blocks
                            .iter()
                            .map(|b| match b {
                                super::ContentBlock::Text { text } => {
                                    ContentBlock::Text { text: text.clone() }
                                }
                                super::ContentBlock::Image { base64, mime_type } => {
                                    ContentBlock::Image {
                                        source: ImageSource {
                                            source_type: "base64".to_string(),
                                            media_type: mime_type.clone(),
                                            data: base64.clone(),
                                        },
                                    }
                                }
                            })
                            .collect()
                    } else {
                        vec![ContentBlock::Text { text: msg.content.clone() }]
                    };

                    // Add tool calls if present
                    if let Some(tool_calls) = &msg.tool_calls {
                        for tc in tool_calls {
                            content.push(ContentBlock::ToolUse {
                                id: tc.id.clone(),
                                name: tc.function.name.clone(),
                                input: serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or_default(),
                            });
                        }
                    }

                    anthropic_messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content,
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
                        index: None,
                        result: None,
                    });
                }
                _ => {}
            }
        }

        CompletionResponse {
            message: Message {
                role: Role::Assistant,
                content: text_content,
                content_blocks: None,
                reasoning_content: None,
                name: None,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
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

    /// Parse Server-Sent Events (SSE) from streaming response
    fn parse_sse_events(text: &str) -> Vec<CompletionChunk> {
        let mut chunks = Vec::new();
        let mut current_text = String::new();

        for line in text.lines() {
            let line = line.trim();

            // SSE events start with "data: "
            if let Some(data) = line.strip_prefix("data: ") {
                // Handle completion
                if data == "[DONE]" {
                    chunks.push(CompletionChunk {
                        content: None,
                        reasoning_content: None,
                        tool_calls: None,
                        is_done: true,
                        usage: None,
                    });
                    break;
                }

                // Parse the event JSON
                match serde_json::from_str::<StreamEvent>(data) {
                    Ok(event) => {
                        match event.event_type.as_str() {
                            "content_block_delta" => {
                                if let Some(delta) = event.delta {
                                    if let Some(text) = delta.text {
                                        current_text.push_str(&text);
                                        chunks.push(CompletionChunk {
                                            content: Some(text),
                                            reasoning_content: None,
                                            tool_calls: None,
                                            is_done: false,
                                            usage: None,
                                        });
                                    }
                                }
                            }
                            "message_stop" => {
                                chunks.push(CompletionChunk {
                                    content: None,
                                    reasoning_content: None,
                                    tool_calls: None,
                                    is_done: true,
                                    usage: None,
                                });
                            }
                            _ => {
                                // Ignore other event types (message_start, content_block_start, etc.)
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Failed to parse stream event: {} - {}", e, data);
                    }
                }
            }
        }

        chunks
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

    fn stream_family(&self) -> ProviderStreamFamily {
        if let Some(family) = self.stream_family_override {
            return family;
        }
        if self.default_model.contains("thinking") {
            ProviderStreamFamily::AnthropicThinking
        } else {
            ProviderStreamFamily::Anthropic
        }
    }

    #[instrument(skip(self, request))]
    async fn complete(&self, request: CompletionRequest) -> crate::Result<CompletionResponse> {
        self.refresh_auth().await?;

        let (system, messages) = Self::to_anthropic_messages(&request.messages);

        let tools = request.tools.as_ref().map(|tools| {
            tools
                .iter()
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

        // Merge provider-specific extra parameters
        let mut body_value = serde_json::to_value(&anthropic_request).unwrap_or_default();
        if let Some(extra) = request.extra {
            if let serde_json::Value::Object(ref mut map) = body_value {
                if let serde_json::Value::Object(extra_map) = extra {
                    for (k, v) in extra_map {
                        map.insert(k, v);
                    }
                }
            }
        }

        debug!("Sending request to Anthropic API");

        let request_url = self.url("/v1/messages");

        // Retry logic for transient errors
        let mut retries = 0;
        let max_retries = 3;

        loop {
            info!("Sending HTTP request (attempt {})", retries + 1);
            match self
                .client
                .post(&request_url)
                .headers(self.headers().await)
                .json(&body_value)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    if !status.is_success() {
                        let text = response.text().await.unwrap_or_default();
                        error!("Anthropic API error: {} - {}", status, text);
                        return Err(crate::error::SyscityError::ExternalService {
                            source: format!("Anthropic API error {}: {}", status, text),
                            cause: None,
                        });
                    }

                    let body = response
                        .text()
                        .await
                        .map_err(crate::error::SyscityError::Http)?;

                    debug!("Received response from Anthropic API");

                    let anthropic_response: AnthropicResponse = serde_json::from_str(&body)
                        .map_err(|e| crate::error::SyscityError::ExternalService {
                            source: format!("Failed to parse Anthropic response: {}", e),
                            cause: Some(Box::new(e)),
                        })?;

                    return Ok(Self::from_anthropic_response(anthropic_response));
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    error!("HTTP request failed (attempt {}): {}", retries + 1, error_msg);

                    // Check if it's a retryable error
                    let is_retryable = error_msg.contains("connection closed")
                        || error_msg.contains("timeout")
                        || error_msg.contains("reset")
                        || error_msg.contains("broken pipe")
                        || error_msg.contains("Connection reset")
                        || error_msg.contains("unexpected EOF");

                    if is_retryable && retries < max_retries {
                        retries += 1;
                        // Exponential backoff: 1s, 2s, 4s
                        let delay = std::time::Duration::from_secs(2_u64.pow(retries as u32 - 1));
                        warn!(
                            "Retryable error detected, retrying after {:?}... (attempt {}/{})",
                            delay, retries, max_retries
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(crate::error::SyscityError::Http(e));
                }
            }
        }
    }

    async fn stream(&self, request: CompletionRequest) -> crate::Result<CompletionStream> {
        self.refresh_auth().await?;

        let (system, messages) = Self::to_anthropic_messages(&request.messages);

        let anthropic_request = AnthropicRequest {
            model: request.model.unwrap_or_else(|| self.default_model.clone()),
            max_tokens: request.max_tokens.unwrap_or(4096),
            system,
            messages,
            tools: request.tools.as_ref().map(|tools| {
                tools
                    .iter()
                    .map(|t| Self::to_anthropic_tool(&t.function))
                    .collect::<Vec<_>>()
            }),
            temperature: request.temperature,
            stream: Some(true),
        };

        // Merge provider-specific extra parameters
        let mut body_value = serde_json::to_value(&anthropic_request).unwrap_or_default();
        if let Some(extra) = request.extra {
            if let serde_json::Value::Object(ref mut map) = body_value {
                if let serde_json::Value::Object(extra_map) = extra {
                    for (k, v) in extra_map {
                        map.insert(k, v);
                    }
                }
            }
        }

        let request_url = format!("{}/v1/messages", self.base_url);

        // Retry logic for transient errors
        let mut retries = 0;
        let max_retries = 3;

        loop {
            match self
                .client
                .post(&request_url)
                .headers(self.headers().await)
                .json(&body_value)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    if !status.is_success() {
                        let body = response.text().await.unwrap_or_default();
                        error!("Anthropic API error: {} - {}", status, body);
                        return Err(crate::error::SyscityError::ExternalService {
                            source: format!("Anthropic API error {}: {}", status, body),
                            cause: None,
                        });
                    }

                    // Process the stream as SSE events
                    let body_stream = response.bytes_stream();
                    let stream = async_stream::stream! {
                        for await chunk in body_stream {
                            match chunk {
                                Ok(bytes) => {
                                    let text = String::from_utf8_lossy(&bytes);
                                    for event in Self::parse_sse_events(&text) {
                                        yield event;
                                    }
                                }
                                Err(e) => {
                                    error!("Stream error: {}", e);
                                }
                            }
                        }
                    };

                    return Ok(Box::pin(stream));
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    error!("HTTP stream request failed (attempt {}): {}", retries + 1, error_msg);

                    let is_retryable = error_msg.contains("connection closed")
                        || error_msg.contains("timeout")
                        || error_msg.contains("reset")
                        || error_msg.contains("broken pipe")
                        || error_msg.contains("Connection reset")
                        || error_msg.contains("unexpected EOF");

                    if is_retryable && retries < max_retries {
                        retries += 1;
                        let delay = std::time::Duration::from_secs(2_u64.pow(retries as u32 - 1));
                        warn!(
                            "Retryable error detected, retrying after {:?}... (attempt {}/{})",
                            delay, retries, max_retries
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(crate::error::SyscityError::ExternalService {
                        source: format!("Anthropic streaming request failed: {}", e),
                        cause: Some(Box::new(e)),
                    });
                }
            }
        }
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
            extra: None,
            ..Default::default()
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
    fn test_to_anthropic_messages_with_image() {
        let messages = vec![Message::user("")
            .with_content_blocks(vec![
                crate::providers::ContentBlock::text("What is this?"),
                crate::providers::ContentBlock::image_base64("abc123", "image/png"),
            ])];

        let (system, anthropic_msgs) = AnthropicProvider::to_anthropic_messages(&messages);

        assert_eq!(system, None);
        assert_eq!(anthropic_msgs.len(), 1);
        assert_eq!(anthropic_msgs[0].role, "user");
        assert_eq!(anthropic_msgs[0].content.len(), 2);
        assert!(matches!(
            &anthropic_msgs[0].content[0],
            ContentBlock::Text { text } if text == "What is this?"
        ));
        assert!(matches!(
            &anthropic_msgs[0].content[1],
            ContentBlock::Image { source } if source.source_type == "base64" && source.media_type == "image/png" && source.data == "abc123"
        ));
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
        assert!(completion.usage.is_some());
        assert_eq!(completion.usage.unwrap().total_tokens, 15);
    }
}
