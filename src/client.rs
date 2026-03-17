//! API Client for connecting to Manta daemon
//!
//! Provides a client for CLI/web commands to connect to the running daemon.

use futures_util::{SinkExt, StreamExt};
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
    pub aliases: Vec<String>,
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
    pub alias: String,
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

    /// Create a new client with custom web port (for WebSocket)
    pub fn with_web_port(host: &str, port: u16, web_port: u16) -> Self {
        Self {
            client: Client::new(),
            base_url: format!("http://{}:{}", host, port),
            ws_url: format!("ws://{}:{}/ws", host, web_port),
        }
    }

    /// Check if daemon is running and has AI agent
    pub async fn health(&self) -> crate::Result<HealthResponse> {
        let url = format!("{}/health", self.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| crate::error::MantaError::Internal(format!("Failed to connect: {}", e)))?;

        let health: HealthResponse = response
            .json()
            .await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;

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
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(crate::error::MantaError::Internal(format!(
                "Server error: {}",
                error_text
            )));
        }

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;

        Ok(chat_response)
    }

    /// Send a chat message via WebSocket
    pub async fn chat_ws(
        &self,
        message: &str,
        conversation_id: Option<&str>,
    ) -> crate::Result<ChatResponse> {
        let url = &self.ws_url;
        let (ws_stream, _) = connect_async(url)
            .await
            .map_err(|e| crate::error::MantaError::Internal(format!("WebSocket connect failed: {}", e)))?;

        let (mut write, mut read) = ws_stream.split();

        // Send message
        let request = ChatRequest {
            message: message.to_string(),
            conversation_id: conversation_id.map(|s| s.to_string()),
        };
        let msg = serde_json::to_string(&request)
            .map_err(|e| crate::error::MantaError::Internal(format!("JSON error: {}", e)))?;

        write.send(Message::Text(msg))
            .await
            .map_err(|e| crate::error::MantaError::Internal(format!("WebSocket send failed: {}", e)))?;

        // Receive response
        if let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let response: ChatResponse = serde_json::from_str(&text)
                        .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
                    Ok(response)
                }
                Ok(Message::Close(_)) => {
                    Err(crate::error::MantaError::Internal("WebSocket closed".to_string()))
                }
                Err(e) => {
                    Err(crate::error::MantaError::Internal(format!("WebSocket error: {}", e)))
                }
                _ => {
                    Err(crate::error::MantaError::Internal("Unexpected message type".to_string()))
                }
            }
        } else {
            Err(crate::error::MantaError::Internal("No response received".to_string()))
        }
    }

    /// Check if daemon is available
    pub async fn is_available(&self) -> bool {
        self.health().await.is_ok()
    }

    /// Get default client using standard daemon address
    pub fn default_client() -> Self {
        Self::with_web_port("127.0.0.1", 18080, 18081)
    }

    // ==================== ADMIN API METHODS ====================

    /// Get Gateway status
    pub async fn get_status(&self) -> crate::Result<GatewayStatus> {
        let url = format!("{}/api/v1/status", self.base_url);
        let response = self.client.get(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let status = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(status)
    }

    /// Get list of providers
    pub async fn get_providers(&self) -> crate::Result<Vec<ProviderInfo>> {
        let url = format!("{}/api/v1/providers", self.base_url);
        let response = self.client.get(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let providers = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(providers)
    }

    /// Get list of model aliases
    pub async fn get_models(&self) -> crate::Result<ModelsResponse> {
        let url = format!("{}/api/v1/models", self.base_url);
        let response = self.client.get(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let models = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(models)
    }

    /// Get default model
    pub async fn get_default_model(&self) -> crate::Result<DefaultModelResponse> {
        let url = format!("{}/api/v1/models/default", self.base_url);
        let response = self.client.get(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let model = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(model)
    }

    /// Switch default model
    pub async fn switch_model(&self, model: &str) -> crate::Result<OperationResult> {
        let url = format!("{}/api/v1/providers/switch", self.base_url);
        let body = serde_json::json!({ "model": model });

        let response = self.client.post(&url).json(&body).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let result = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(result)
    }

    /// Enable a provider
    pub async fn enable_provider(&self, provider: &str) -> crate::Result<OperationResult> {
        let url = format!("{}/api/v1/providers/{}/enable", self.base_url, provider);
        let response = self.client.post(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let result = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(result)
    }

    /// Disable a provider
    pub async fn disable_provider(&self, provider: &str) -> crate::Result<OperationResult> {
        let url = format!("{}/api/v1/providers/{}/disable", self.base_url, provider);
        let response = self.client.post(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let result = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(result)
    }

    /// Check provider health
    pub async fn check_provider_health(&self, provider: &str) -> crate::Result<HealthCheckResponse> {
        let url = format!("{}/api/v1/providers/{}/check", self.base_url, provider);
        let response = self.client.post(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let result = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(result)
    }

    /// Get fallback chain for an alias
    pub async fn get_fallback_chain(&self, alias: &str) -> crate::Result<FallbackChainResponse> {
        let url = format!("{}/api/v1/providers/fallback/{}", self.base_url, alias);
        let response = self.client.get(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let result = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(result)
    }

    /// Get list of agents
    pub async fn get_agents(&self) -> crate::Result<Vec<AgentInfo>> {
        let url = format!("{}/api/v1/agents", self.base_url);
        let response = self.client.get(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let agents = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
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
            "model_alias": model,
        });

        let response = self.client.post(&url).json(&body).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        let result = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(result)
    }

    /// Get chat history for a conversation
    pub async fn get_chat_history(&self, conversation_id: &str, limit: usize) -> crate::Result<ChatHistoryResponse> {
        let url = format!("{}/api/v1/conversations/{}/messages?limit={}", self.base_url, conversation_id, limit);
        let response = self.client.get(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(crate::error::MantaError::Internal(format!(
                "Failed to get chat history: {}",
                response.status()
            )));
        }

        let history = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(history)
    }

    /// Get last conversation ID for a user
    pub async fn get_last_conversation(&self, user_id: &str) -> crate::Result<LastConversationResponse> {
        let url = format!("{}/api/v1/conversations/last?user_id={}", self.base_url, user_id);
        let response = self.client.get(&url).send().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(crate::error::MantaError::Internal(format!(
                "Failed to get last conversation: {}",
                response.status()
            )));
        }

        let result = response.json().await
            .map_err(|e| crate::error::MantaError::Internal(format!("Invalid response: {}", e)))?;
        Ok(result)
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
                Err(crate::error::MantaError::Internal(
                    "Daemon is running but AI agent is not configured.\n\
                     Set MANTA_BASE_URL and MANTA_API_KEY, then restart daemon.".to_string()
                ))
            }
        }
        Err(_) => {
            Err(crate::error::MantaError::Internal(
                "Daemon is not running.\n\
                 Start it with: manta start".to_string()
            ))
        }
    }
}
