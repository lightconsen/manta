//! Assistant Mesh - Inter-Assistant Communication
//!
//! This module implements message routing and broadcasting between assistants.
//! It enables assistants to communicate with each other in a mesh topology.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// A message in the mesh
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshMessage {
    /// Unique message ID
    pub id: String,
    /// Sender assistant ID
    pub from: String,
    /// Recipient assistant ID (None for broadcast)
    pub to: Option<String>,
    /// Message content
    pub content: String,
    /// Message type
    pub msg_type: MessageType,
    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Reply to message ID (for threading)
    pub reply_to: Option<String>,
    /// Message metadata
    #[serde(skip)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Message types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    /// Direct message
    Direct,
    /// Broadcast to all assistants
    Broadcast,
    /// Request/response pattern
    Request,
    /// Response to a request
    Response,
    /// Event notification
    Event,
}

impl MeshMessage {
    /// Create a new direct message
    pub fn direct(from: impl Into<String>, to: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            from: from.into(),
            to: Some(to.into()),
            content: content.into(),
            msg_type: MessageType::Direct,
            timestamp: chrono::Utc::now(),
            reply_to: None,
            metadata: HashMap::new(),
        }
    }

    /// Create a broadcast message
    pub fn broadcast(from: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            from: from.into(),
            to: None,
            content: content.into(),
            msg_type: MessageType::Broadcast,
            timestamp: chrono::Utc::now(),
            reply_to: None,
            metadata: HashMap::new(),
        }
    }

    /// Create a request message
    pub fn request(from: impl Into<String>, to: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            from: from.into(),
            to: Some(to.into()),
            content: content.into(),
            msg_type: MessageType::Request,
            timestamp: chrono::Utc::now(),
            reply_to: None,
            metadata: HashMap::new(),
        }
    }

    /// Create a response to this message
    pub fn respond(&self, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            from: self.to.clone().unwrap_or_default(),
            to: Some(self.from.clone()),
            content: content.into(),
            msg_type: MessageType::Response,
            timestamp: chrono::Utc::now(),
            reply_to: Some(self.id.clone()),
            metadata: HashMap::new(),
        }
    }
}

/// Mesh router for inter-assistant communication
#[derive(Debug)]
pub struct AssistantMesh {
    /// Registered assistants and their message channels
    routes: Arc<RwLock<HashMap<String, tokio::sync::mpsc::UnboundedSender<MeshMessage>>>>,
    /// Message history (recent messages)
    history: Arc<RwLock<Vec<MeshMessage>>>,
    /// Maximum history size
    max_history: usize,
}

impl AssistantMesh {
    /// Create a new mesh router
    pub fn new() -> Self {
        Self::with_history_size(1000)
    }

    /// Create with custom history size
    pub fn with_history_size(max_history: usize) -> Self {
        Self {
            routes: Arc::new(RwLock::new(HashMap::new())),
            history: Arc::new(RwLock::new(Vec::new())),
            max_history,
        }
    }

    /// Register an assistant with the mesh
    pub async fn register(&self, assistant_id: impl Into<String>) -> tokio::sync::mpsc::UnboundedReceiver<MeshMessage> {
        let assistant_id = assistant_id.into();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        let mut routes = self.routes.write().await;
        routes.insert(assistant_id.clone(), tx);

        info!("Registered assistant {} with mesh", assistant_id);
        rx
    }

    /// Unregister an assistant
    pub async fn unregister(&self, assistant_id: &str) {
        let mut routes = self.routes.write().await;
        routes.remove(assistant_id);
        info!("Unregistered assistant {} from mesh", assistant_id);
    }

    /// Route a message to a specific assistant
    pub async fn route(&self, message: MeshMessage) -> crate::Result<()> {
        let recipient = message.to.clone();
        let msg_type = message.msg_type;
        let from = message.from.clone();

        // Store in history
        self.add_to_history(message.clone()).await;

        if let Some(recipient_id) = recipient {
            // Direct message
            let routes = self.routes.read().await;
            if let Some(sender) = routes.get(&recipient_id) {
                sender.send(message).map_err(|_| {
                    crate::error::MantaError::Internal("Failed to send message".to_string())
                })?;
                debug!("Routed message to {}", recipient_id);
            } else {
                warn!("Recipient {} not found in mesh", recipient_id);
                return Err(crate::error::MantaError::NotFound {
                    resource: format!("Assistant {}", recipient_id)
                });
            }
        } else if msg_type == MessageType::Broadcast {
            // Broadcast
            let routes = self.routes.read().await;
            for (id, sender) in routes.iter() {
                if id != &from {
                    let _ = sender.send(message.clone());
                }
            }
            debug!("Broadcasted message to {} assistants", routes.len().saturating_sub(1));
        }

        Ok(())
    }

    /// Send a message from one assistant to another
    pub async fn send(
        &self,
        from: impl Into<String>,
        to: impl Into<String>,
        content: impl Into<String>,
    ) -> crate::Result<String> {
        let message = MeshMessage::direct(from, to, content);
        let msg_id = message.id.clone();
        self.route(message).await?;
        Ok(msg_id)
    }

    /// Broadcast a message to all assistants
    pub async fn broadcast(
        &self,
        from: impl Into<String>,
        content: impl Into<String>,
    ) -> crate::Result<String> {
        let message = MeshMessage::broadcast(from, content);
        let msg_id = message.id.clone();
        self.route(message).await?;
        Ok(msg_id)
    }

    /// List all registered assistants
    pub async fn list_registered(&self) -> Vec<String> {
        let routes = self.routes.read().await;
        routes.keys().cloned().collect()
    }

    /// Check if an assistant is registered
    pub async fn is_registered(&self, assistant_id: &str) -> bool {
        let routes = self.routes.read().await;
        routes.contains_key(assistant_id)
    }

    /// Get message history
    pub async fn get_history(&self) -> Vec<MeshMessage> {
        let history = self.history.read().await;
        history.clone()
    }

    /// Get messages for a specific assistant
    pub async fn get_messages_for(&self, assistant_id: &str) -> Vec<MeshMessage> {
        let history = self.history.read().await;
        history
            .iter()
            .filter(|m| m.from == assistant_id || m.to.as_ref() == Some(&assistant_id.to_string()))
            .cloned()
            .collect()
    }

    /// Add message to history
    async fn add_to_history(&self, message: MeshMessage) {
        let mut history = self.history.write().await;
        history.push(message);

        // Trim history if needed
        if history.len() > self.max_history {
            let excess = history.len() - self.max_history;
            history.drain(0..excess);
        }
    }

    /// Get mesh statistics
    pub async fn stats(&self) -> MeshStats {
        let routes = self.routes.read().await;
        let history = self.history.read().await;

        MeshStats {
            registered_assistants: routes.len(),
            total_messages: history.len(),
            broadcasts: history.iter().filter(|m| m.msg_type == MessageType::Broadcast).count(),
            direct_messages: history.iter().filter(|m| m.msg_type == MessageType::Direct).count(),
            requests: history.iter().filter(|m| m.msg_type == MessageType::Request).count(),
            responses: history.iter().filter(|m| m.msg_type == MessageType::Response).count(),
        }
    }
}

impl Default for AssistantMesh {
    fn default() -> Self {
        Self::new()
    }
}

/// Mesh statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshStats {
    /// Number of registered assistants
    pub registered_assistants: usize,
    /// Total messages processed
    pub total_messages: usize,
    /// Number of broadcasts
    pub broadcasts: usize,
    /// Number of direct messages
    pub direct_messages: usize,
    /// Number of requests
    pub requests: usize,
    /// Number of responses
    pub responses: usize,
}

/// Tool for mesh operations
pub mod tool {
    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::tools::{Tool, ToolContext, ToolExecutionResult};

    /// Tool for mesh communication
    #[derive(Debug)]
    pub struct MeshTool {
        mesh: AssistantMesh,
    }

    impl MeshTool {
        /// Create a new mesh tool
        pub fn new(mesh: AssistantMesh) -> Self {
            Self { mesh }
        }
    }

    #[async_trait]
    impl Tool for MeshTool {
        fn name(&self) -> &str {
            "mesh"
        }

        fn description(&self) -> &str {
            r#"Send messages between assistants in the mesh network.

Use this to:
- Send direct messages to other assistants
- Broadcast messages to all assistants
- Query mesh status and statistics

The mesh enables assistants to communicate and coordinate.
Messages are routed automatically to recipients."#
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["send", "broadcast", "status", "list"],
                        "description": "Action to perform"
                    },
                    "to": {
                        "type": "string",
                        "description": "Recipient assistant ID (for send)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Message content"
                    }
                },
                "required": ["action"]
            })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
            context: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            let action = args["action"]
                .as_str()
                .ok_or_else(|| crate::error::MantaError::Validation("action is required".to_string()))?;

            let from = context.conversation_id.clone();

            match action {
                "send" => {
                    let to = args["to"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "to is required for send".to_string()
                        ))?;
                    let content = args["content"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "content is required for send".to_string()
                        ))?;

                    let msg_id = self.mesh.send(&from, to, content).await?;

                    Ok(ToolExecutionResult::success(format!(
                        "Message sent to {}", to
                    )).with_data(json!({"message_id": msg_id, "to": to})))
                }

                "broadcast" => {
                    let content = args["content"]
                        .as_str()
                        .ok_or_else(|| crate::error::MantaError::Validation(
                            "content is required for broadcast".to_string()
                        ))?;

                    let msg_id = self.mesh.broadcast(&from, content).await?;

                    Ok(ToolExecutionResult::success(format!(
                        "Message broadcast to all assistants"
                    )).with_data(json!({"message_id": msg_id})))
                }

                "status" => {
                    let stats = self.mesh.stats().await;
                    let registered = self.mesh.list_registered().await;

                    Ok(ToolExecutionResult::success(format!(
                        "{} assistants registered in mesh",
                        stats.registered_assistants
                    )).with_data(json!({
                        "stats": stats,
                        "registered": registered,
                    })))
                }

                "list" => {
                    let registered = self.mesh.list_registered().await;

                    Ok(ToolExecutionResult::success(format!(
                        "{} assistants registered",
                        registered.len()
                    )).with_data(json!({"assistants": registered})))
                }

                _ => Err(crate::error::MantaError::Validation(format!(
                    "Unknown action: {}", action
                ))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mesh_message_creation() {
        let msg = MeshMessage::direct("assistant1", "assistant2", "Hello");
        assert_eq!(msg.from, "assistant1");
        assert_eq!(msg.to, Some("assistant2".to_string()));
        assert_eq!(msg.content, "Hello");
        assert_eq!(msg.msg_type, MessageType::Direct);
    }

    #[test]
    fn test_broadcast_message() {
        let msg = MeshMessage::broadcast("assistant1", "Hello all");
        assert_eq!(msg.from, "assistant1");
        assert_eq!(msg.to, None);
        assert_eq!(msg.msg_type, MessageType::Broadcast);
    }

    #[test]
    fn test_message_response() {
        let request = MeshMessage::request("a1", "a2", "Help");
        let response = request.respond("Here you go");

        assert_eq!(response.from, "a2");
        assert_eq!(response.to, Some("a1".to_string()));
        assert_eq!(response.reply_to, Some(request.id));
        assert_eq!(response.msg_type, MessageType::Response);
    }

    #[tokio::test]
    async fn test_mesh_registration() {
        let mesh = AssistantMesh::new();

        let _rx = mesh.register("assistant1").await;
        assert!(mesh.is_registered("assistant1").await);

        let registered = mesh.list_registered().await;
        assert_eq!(registered.len(), 1);
        assert_eq!(registered[0], "assistant1");

        mesh.unregister("assistant1").await;
        assert!(!mesh.is_registered("assistant1").await);
    }

    #[tokio::test]
    async fn test_mesh_routing() {
        let mesh = AssistantMesh::new();

        let mut rx = mesh.register("assistant2").await;

        let msg = MeshMessage::direct("assistant1", "assistant2", "Hello");
        mesh.route(msg).await.unwrap();

        let received = rx.try_recv();
        assert!(received.is_ok());
        assert_eq!(received.unwrap().content, "Hello");
    }
}
