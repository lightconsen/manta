//! API Client for connecting to Syscity daemon
//!
//! Provides a client for CLI/web commands to connect to the running daemon.

use futures::{SinkExt, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

/// Daemon API client
#[derive(Clone)]
pub struct DaemonClient {
    client: Client,
    base_url: String,
    ws_url: String,
}

/// Chat request
#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub message: String,
    pub conversation_id: Option<String>,
}

/// Chat response
#[derive(Debug, Deserialize)]
pub struct ChatResponse {
    pub response: String,
    pub conversation_id: String,
}

/// Health response
#[derive(Debug, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub agent: String,
}

/// Gateway status response
#[derive(Debug, Deserialize)]
pub struct GatewayStatus {
    pub agents: AgentStatus,
    pub channels: usize,
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct AgentStatus {
    pub total: usize,
    pub busy: usize,
}

/// Provider info response
#[derive(Debug, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub provider_type: String,
    pub enabled: bool,
    pub health: ProviderHealthInfo,
}

#[derive(Debug, Deserialize)]
pub struct ProviderHealthInfo {
    pub state: String,
    pub failures: u32,
    pub successes: u64,
}

/// Models list response
#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    pub models: Vec<String>,
}

/// Default model response
#[derive(Debug, Deserialize)]
pub struct DefaultModelResponse {
    pub default_model: String,
}

/// Generic operation result
#[derive(Debug, Deserialize)]
pub struct OperationResult {
    pub success: bool,
    pub message: String,
    pub error: Option<String>,
}

/// Fallback chain response
#[derive(Debug, Deserialize)]
pub struct FallbackChainResponse {
    pub model_id: String,
    pub fallback_chain: Vec<String>,
}

/// Health check response
#[derive(Debug, Deserialize)]
pub struct HealthCheckResponse {
    pub provider: String,
    pub healthy: bool,
    pub checked_at: String,
}

/// Send message response
#[derive(Debug, Deserialize)]
pub struct SendMessageResponse {
    pub message_id: String,
    pub session_id: String,
    pub response: Option<String>,
    pub queued: bool,
    pub status: String,
}

/// Chat message in conversation history
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub conversation_id: String,
    pub user_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// Conversation history response
#[derive(Debug, Deserialize)]
pub struct ChatHistoryResponse {
    pub messages: Vec<ChatMessage>,
    pub conversation_id: String,
}

/// Last conversation response
#[derive(Debug, Deserialize)]
pub struct LastConversationResponse {
    pub conversation_id: Option<String>,
}

/// Agent list item
#[derive(Debug, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub busy: bool,
}

impl DaemonClient {
    /// Create a new client
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            client: Client::new(),
            base_url: format!("http://{}:{}", host, port),
            ws_url: format!("ws://{}:{}/chat/stream", host, port),
        }
    }

    /// Create a new client connecting to the unified gateway port
    pub fn with_ws(host: &str, port: u16) -> Self {
        Self {
            client: Client::new(),
            base_url: format!("http://{}:{}", host, port),
            ws_url: format!("ws://{}:{}/ws", host, port),
        }
    }

    /// Check if daemon is running and has AI agent
    pub async fn health(&self) -> crate::Result<HealthResponse> {
        let url = format!("{}/health", self.base_url);
        let response = self.client.get(&url).send().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to connect: {}", e))
        })?;

        let health: HealthResponse = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;

        Ok(health)
    }

    /// Send a chat message to the daemon via HTTP
    pub async fn chat(
        &self,
        message: &str,
        conversation_id: Option<&str>,
    ) -> crate::Result<ChatResponse> {
        let url = format!("{}/chat", self.base_url);
        let request = ChatRequest {
            message: message.to_string(),
            conversation_id: conversation_id.map(|s| s.to_string()),
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::Internal(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(crate::error::SyscityError::Internal(format!(
                "Server error: {}",
                error_text
            )));
        }

        let chat_response: ChatResponse = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;

        Ok(chat_response)
    }

    /// Send a chat message via WebSocket
    pub async fn chat_ws(
        &self,
        message: &str,
        conversation_id: Option<&str>,
    ) -> crate::Result<ChatResponse> {
        let url = &self.ws_url;
        let (ws_stream, _) = connect_async(url).await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("WebSocket connect failed: {}", e))
        })?;

        let (mut write, mut read) = ws_stream.split();

        // Send message
        let request = ChatRequest {
            message: message.to_string(),
            conversation_id: conversation_id.map(|s| s.to_string()),
        };
        let msg = serde_json::to_string(&request)
            .map_err(|e| crate::error::SyscityError::Internal(format!("JSON error: {}", e)))?;

        write.send(Message::Text(msg)).await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("WebSocket send failed: {}", e))
        })?;

        // Receive response
        if let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let response: ChatResponse = serde_json::from_str(&text).map_err(|e| {
                        crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
                    })?;
                    Ok(response)
                }
                Ok(Message::Close(_)) => {
                    Err(crate::error::SyscityError::Internal("WebSocket closed".to_string()))
                }
                Err(e) => {
                    Err(crate::error::SyscityError::Internal(format!("WebSocket error: {}", e)))
                }
                _ => {
                    Err(crate::error::SyscityError::Internal("Unexpected message type".to_string()))
                }
            }
        } else {
            Err(crate::error::SyscityError::Internal("No response received".to_string()))
        }
    }

    /// Generic WebSocket RPC call: connects, performs the `connect` handshake,
    /// sends `method` with `params`, and returns the response payload.
    ///
    /// The gateway's WS protocol expects the `connect` frame first
    /// (`{protocol_version: 1}`), then a request frame; the response is a
    /// `WsResponse` with `ok`/`payload`/`error`.
    pub async fn ws_call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        use serde_json::json;

        let (ws_stream, _) = connect_async(&self.ws_url).await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("WebSocket connect failed: {}", e))
        })?;
        let (mut write, mut read) = ws_stream.split();

        async fn send_frame<W>(
            write: &mut W,
            id: &str,
            method: &str,
            params: &serde_json::Value,
        ) -> crate::Result<()>
        where
            W: futures::Sink<Message> + Unpin,
            W::Error: std::fmt::Display,
        {
            let frame = json!({ "type": "req", "id": id, "method": method, "params": params });
            let text = serde_json::to_string(&frame)
                .map_err(|e| crate::error::SyscityError::Internal(format!("JSON error: {}", e)))?;
            futures::SinkExt::send(write, Message::Text(text))
                .await
                .map_err(|e| {
                    crate::error::SyscityError::Internal(format!("WebSocket send failed: {}", e))
                })
        }

        async fn read_resp<R>(read: &mut R) -> crate::Result<serde_json::Value>
        where
            R: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
                + Unpin,
        {
            if let Some(msg) = futures::StreamExt::next(read).await {
                match msg {
                    Ok(Message::Text(text)) => Ok(serde_json::from_str::<serde_json::Value>(&text)
                        .map_err(|e| {
                            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
                        })?),
                    Ok(Message::Close(_)) => {
                        Err(crate::error::SyscityError::Internal("WebSocket closed".to_string()))
                    }
                    Err(e) => {
                        Err(crate::error::SyscityError::Internal(format!("WebSocket error: {}", e)))
                    }
                    _ => Err(crate::error::SyscityError::Internal(
                        "Unexpected message type".to_string(),
                    )),
                }
            } else {
                Err(crate::error::SyscityError::Internal("No response received".to_string()))
            }
        }

        // Connect handshake.
        send_frame(&mut write, "conn", "connect", &json!({ "protocol_version": 1 })).await?;
        loop {
            let resp = read_resp(&mut read).await?;
            if resp["id"].as_str() == Some("conn") {
                break;
            }
        }

        // Send the actual method and read its response.
        send_frame(&mut write, "req", method, &params).await?;
        loop {
            let resp = read_resp(&mut read).await?;
            if resp["id"].as_str() != Some("req") {
                continue;
            }
            if resp["ok"].as_bool().unwrap_or(false) {
                return Ok(resp
                    .get("payload")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null));
            }
            let msg = resp
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("gateway error");
            return Err(crate::error::SyscityError::Internal(msg.to_string()));
        }
    }

    /// Check if daemon is available
    pub async fn is_available(&self) -> bool {
        self.health().await.is_ok()
    }

    /// Get default client using standard daemon address
    pub fn default_client() -> Self {
        Self::with_ws("127.0.0.1", 18080)
    }

    // ==================== ADMIN API METHODS ====================

    /// Get Gateway status
    pub async fn get_status(&self) -> crate::Result<GatewayStatus> {
        let url = format!("{}/api/v1/status", self.base_url);
        let response =
            self.client.get(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        let status = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(status)
    }

    /// Get list of providers
    pub async fn get_providers(&self) -> crate::Result<Vec<ProviderInfo>> {
        let url = format!("{}/api/v1/providers", self.base_url);
        let response =
            self.client.get(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        let providers = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(providers)
    }

    /// Get list of models
    pub async fn get_models(&self) -> crate::Result<ModelsResponse> {
        let url = format!("{}/api/v1/models", self.base_url);
        let response =
            self.client.get(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        let models = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(models)
    }

    /// Get default model
    pub async fn get_default_model(&self) -> crate::Result<DefaultModelResponse> {
        let url = format!("{}/api/v1/models/default", self.base_url);
        let response =
            self.client.get(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        let model = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(model)
    }

    /// Switch default model
    pub async fn switch_model(&self, model: &str) -> crate::Result<OperationResult> {
        let url = format!("{}/api/v1/providers/switch", self.base_url);
        let body = serde_json::json!({ "model": model });

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::Internal(format!("Request failed: {}", e)))?;

        let result = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(result)
    }

    /// Enable a provider
    pub async fn enable_provider(&self, provider: &str) -> crate::Result<OperationResult> {
        let url = format!("{}/api/v1/providers/{}/enable", self.base_url, provider);
        let response =
            self.client.post(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        let result = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(result)
    }

    /// Disable a provider
    pub async fn disable_provider(&self, provider: &str) -> crate::Result<OperationResult> {
        let url = format!("{}/api/v1/providers/{}/disable", self.base_url, provider);
        let response =
            self.client.post(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        let result = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(result)
    }

    /// Check provider health
    pub async fn check_provider_health(
        &self,
        provider: &str,
    ) -> crate::Result<HealthCheckResponse> {
        let url = format!("{}/api/v1/providers/{}/check", self.base_url, provider);
        let response =
            self.client.post(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        let result = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(result)
    }

    /// Get fallback chain for a model ID
    pub async fn get_fallback_chain(&self, model_id: &str) -> crate::Result<FallbackChainResponse> {
        let url = format!("{}/api/v1/providers/fallback/{}", self.base_url, model_id);
        let response =
            self.client.get(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        let result = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(result)
    }

    /// Get list of agents
    pub async fn get_agents(&self) -> crate::Result<Vec<AgentInfo>> {
        let url = format!("{}/api/v1/agents", self.base_url);
        let response =
            self.client.get(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        let agents = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(agents)
    }

    /// Send a message with optional provider/model override
    pub async fn send_message_with_override(
        &self,
        session_id: &str,
        message: &str,
        provider: Option<String>,
        model: Option<String>,
    ) -> crate::Result<SendMessageResponse> {
        let url = format!("{}/api/v1/sessions/{}/messages", self.base_url, session_id);
        let body = serde_json::json!({
            "message": message,
            "provider_override": provider,
            "model_id": model,
        });

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| crate::error::SyscityError::Internal(format!("Request failed: {}", e)))?;

        let result = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(result)
    }

    /// Get chat history for a conversation
    pub async fn get_chat_history(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> crate::Result<ChatHistoryResponse> {
        let url = format!(
            "{}/api/v1/conversations/{}/messages?limit={}",
            self.base_url, conversation_id, limit
        );
        let response =
            self.client.get(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(crate::error::SyscityError::Internal(format!(
                "Failed to get chat history: {}",
                response.status()
            )));
        }

        let history = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(history)
    }

    /// Get last conversation ID for a user
    pub async fn get_last_conversation(
        &self,
        user_id: &str,
    ) -> crate::Result<LastConversationResponse> {
        let url = format!("{}/api/v1/conversations/last?user_id={}", self.base_url, user_id);
        let response =
            self.client.get(&url).send().await.map_err(|e| {
                crate::error::SyscityError::Internal(format!("Request failed: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(crate::error::SyscityError::Internal(format!(
                "Failed to get last conversation: {}",
                response.status()
            )));
        }

        let result = response.json().await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Invalid response: {}", e))
        })?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_client_new() {
        let client = DaemonClient::new("127.0.0.1", 18080);
        assert_eq!(client.base_url, "http://127.0.0.1:18080");
        assert_eq!(client.ws_url, "ws://127.0.0.1:18080/chat/stream");
    }

    #[test]
    fn test_daemon_client_with_ws() {
        let client = DaemonClient::with_ws("127.0.0.1", 18080);
        assert_eq!(client.base_url, "http://127.0.0.1:18080");
        assert_eq!(client.ws_url, "ws://127.0.0.1:18080/ws");
    }

    #[test]
    fn test_daemon_client_default_client() {
        let client = DaemonClient::default_client();
        assert_eq!(client.base_url, "http://127.0.0.1:18080");
        assert_eq!(client.ws_url, "ws://127.0.0.1:18080/ws");
    }

    #[test]
    fn test_chat_request_serialize() {
        let req = ChatRequest {
            message: "hello".to_string(),
            conversation_id: Some("conv-1".to_string()),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["message"], "hello");
        assert_eq!(json["conversation_id"], "conv-1");
    }

    #[test]
    fn test_chat_response_deserialize() {
        let json = r#"{"response":"Hi there","conversation_id":"conv-1"}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.response, "Hi there");
        assert_eq!(resp.conversation_id, "conv-1");
    }

    #[test]
    fn test_health_response_deserialize() {
        let json = r#"{"status":"ok","agent":"ready"}"#;
        let health: HealthResponse = serde_json::from_str(json).unwrap();
        assert_eq!(health.status, "ok");
        assert_eq!(health.agent, "ready");
    }

    #[test]
    fn test_gateway_status_deserialize() {
        let json = r#"{"agents":{"total":5,"busy":2},"channels":3,"version":"1.0.0"}"#;
        let status: GatewayStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status.agents.total, 5);
        assert_eq!(status.agents.busy, 2);
        assert_eq!(status.channels, 3);
        assert_eq!(status.version, "1.0.0");
    }

    #[test]
    fn test_provider_info_deserialize() {
        let json = r#"{"name":"openai","provider_type":"openai","enabled":true,"health":{"state":"healthy","failures":0,"successes":100}}"#;
        let info: ProviderInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.name, "openai");
        assert!(info.enabled);
        assert_eq!(info.health.state, "healthy");
    }

    #[test]
    fn test_models_response_deserialize() {
        let json = r#"{"models":["gpt-4","gpt-3.5"]}"#;
        let resp: ModelsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.models, vec!["gpt-4", "gpt-3.5"]);
    }

    #[test]
    fn test_default_model_response_deserialize() {
        let json = r#"{"default_model":"gpt-4"}"#;
        let resp: DefaultModelResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.default_model, "gpt-4");
    }

    #[test]
    fn test_operation_result_deserialize() {
        let json = r#"{"success":true,"message":"Done","error":null}"#;
        let result: OperationResult = serde_json::from_str(json).unwrap();
        assert!(result.success);
        assert_eq!(result.message, "Done");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_health_check_response_deserialize() {
        let json = r#"{"provider":"openai","healthy":true,"checked_at":"2024-01-01T00:00:00Z"}"#;
        let resp: HealthCheckResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.provider, "openai");
        assert!(resp.healthy);
    }

    #[test]
    fn test_chat_message_serde_roundtrip() {
        let msg = ChatMessage {
            id: "msg-1".to_string(),
            conversation_id: "conv-1".to_string(),
            user_id: "user-1".to_string(),
            role: "user".to_string(),
            content: "Hello".to_string(),
            created_at: "2024-01-01".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, msg.id);
        assert_eq!(decoded.content, msg.content);
    }

    #[test]
    fn test_chat_history_response_deserialize() {
        let json = r#"{"messages":[],"conversation_id":"conv-1"}"#;
        let resp: ChatHistoryResponse = serde_json::from_str(json).unwrap();
        assert!(resp.messages.is_empty());
        assert_eq!(resp.conversation_id, "conv-1");
    }

    #[test]
    fn test_last_conversation_response_deserialize() {
        let json = r#"{"conversation_id":"conv-1"}"#;
        let resp: LastConversationResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.conversation_id, Some("conv-1".to_string()));
    }

    #[test]
    fn test_fallback_chain_response_deserialize() {
        let json = r#"{"model_id":"gpt-4o","fallback_chain":["primary","secondary"]}"#;
        let resp: FallbackChainResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.model_id, "gpt-4o");
        assert_eq!(resp.fallback_chain, vec!["primary", "secondary"]);
    }

    #[test]
    fn test_send_message_response_deserialize() {
        let json = r#"{"message_id":"msg-1","session_id":"sess-1","response":null,"queued":false,"status":"sent"}"#;
        let resp: SendMessageResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.message_id, "msg-1");
        assert_eq!(resp.session_id, "sess-1");
        assert!(resp.response.is_none());
        assert!(!resp.queued);
    }
}

/// Check if daemon is running, returning helpful error if not
pub async fn check_daemon() -> crate::Result<DaemonClient> {
    let client = DaemonClient::default_client();

    match client.health().await {
        Ok(health) => {
            if health.agent == "ready" {
                Ok(client)
            } else {
                Err(crate::error::SyscityError::Internal(
                    "Daemon is running but AI agent is not configured.\nSet SYSCITY_BASE_URL and \
                     SYSCITY_API_KEY, then restart daemon."
                        .to_string(),
                ))
            }
        }
        Err(_) => Err(crate::error::SyscityError::Internal(
            "Daemon is not running.\nStart it with: syscity start".to_string(),
        )),
    }
}
