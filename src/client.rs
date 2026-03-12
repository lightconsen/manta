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
        Self::with_web_port("127.0.0.1", 3000, 8080)
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
