//! Channel-ACP Binding Integration
//!
//! Bridges channel conversations to ACP (Agent Control Plane) sessions,
//! allowing channel users to spawn, manage, and communicate with subagents.
//!
//! Flow:
//! 1. Channel message arrives with ACP intent (e.g., `/spawn`, `/acp` command)
//! 2. `ChannelAcpBridge` forwards the message to the ACP control plane
//! 3. ACP executes the message in a new or existing subagent session
//! 4. ACP response is routed back through the channel's outbound path

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::info;

use crate::acp::{AcpCommand, AcpSessionId, AcpSessionStatus, ExecutionMode};
use crate::channels::{ConversationId, IncomingMessage, OutgoingMessage, UserId};

/// A binding between a channel conversation and an ACP session.
#[derive(Debug, Clone)]
pub struct ChannelAcpBinding {
    /// The ACP session ID.
    pub acp_session_id: AcpSessionId,
    /// The channel conversation ID.
    pub channel_conversation_id: ConversationId,
    /// The channel name (e.g., "telegram", "discord").
    pub channel_name: String,
    /// The user who initiated the binding.
    pub user_id: UserId,
    /// Execution mode for this binding.
    pub mode: ExecutionMode,
    /// When the binding was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl ChannelAcpBinding {
    /// Create a new binding.
    pub fn new(
        acp_session_id: AcpSessionId,
        channel_conversation_id: ConversationId,
        channel_name: impl Into<String>,
        user_id: UserId,
        mode: ExecutionMode,
    ) -> Self {
        Self {
            acp_session_id,
            channel_conversation_id,
            channel_name: channel_name.into(),
            user_id,
            mode,
            created_at: chrono::Utc::now(),
        }
    }
}

/// Result of forwarding a message to ACP.
#[derive(Debug, Clone)]
pub enum AcpForwardResult {
    /// Message was forwarded and a response is available.
    Completed(Box<OutgoingMessage>),
    /// Forwarding failed.
    Failed(String),
}

/// Bridge that connects channel messages to the ACP system.
#[derive(Clone)]
pub struct ChannelAcpBridge {
    /// ACP command sender (to forward messages to ACP).
    acp_tx: mpsc::Sender<AcpCommand>,
    /// Active bindings: channel_conversation_id -> binding.
    bindings: Arc<RwLock<HashMap<String, ChannelAcpBinding>>>,
    /// Reverse lookup: acp_session_id -> channel_conversation_id.
    session_to_channel: Arc<RwLock<HashMap<String, String>>>,
}

impl ChannelAcpBridge {
    /// Create a new bridge.
    pub fn new(acp_tx: mpsc::Sender<AcpCommand>) -> Self {
        Self {
            acp_tx,
            bindings: Arc::new(RwLock::new(HashMap::new())),
            session_to_channel: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Forward an incoming channel message to ACP for execution.
    ///
    /// If the conversation already has an active binding, the message is
    /// forwarded to the existing ACP session. Otherwise, a new session is
    /// created.
    pub async fn forward_message(
        &self,
        message: IncomingMessage,
        mode: ExecutionMode,
        channel_name: &str,
    ) -> AcpForwardResult {
        let conv_id = message.conversation_id.0.clone();

        // Check for existing binding
        let existing_session_id = {
            let bindings = self.bindings.read().await;
            bindings.get(&conv_id).map(|b| b.acp_session_id.clone())
        };

        let session_id = match existing_session_id {
            Some(sid) => sid,
            None => {
                // Create new binding
                let acp_session_id = AcpSessionId::new();
                let binding = ChannelAcpBinding::new(
                    acp_session_id.clone(),
                    ConversationId::new(&conv_id),
                    channel_name,
                    message.user_id.clone(),
                    mode,
                );

                let mut bindings = self.bindings.write().await;
                let mut session_map = self.session_to_channel.write().await;

                bindings.insert(conv_id.clone(), binding);
                session_map.insert(acp_session_id.0.clone(), conv_id);

                acp_session_id
            }
        };

        // Execute the message through the ACP actor, using the configured
        // default agent builder to resolve an agent.
        let (respond_tx, respond_rx) = oneshot::channel();
        let cmd = AcpCommand::ExecuteForBridge {
            session_id: session_id.0.clone(),
            message,
            mode,
            respond_to: respond_tx,
        };

        if let Err(e) = self.acp_tx.send(cmd).await {
            return AcpForwardResult::Failed(format!("ACP send failed: {}", e));
        }

        match tokio::time::timeout(std::time::Duration::from_secs(30), respond_rx).await {
            Ok(Ok(Ok(response))) => AcpForwardResult::Completed(Box::new(response)),
            Ok(Ok(Err(e))) => AcpForwardResult::Failed(format!("ACP execution failed: {}", e)),
            Ok(Err(_)) => AcpForwardResult::Failed("ACP response channel closed".to_string()),
            Err(_) => AcpForwardResult::Failed("ACP execution timed out".to_string()),
        }
    }

    /// Send a message to a specific ACP session from a channel conversation.
    pub async fn send_to_session(
        &self,
        acp_session_id: &AcpSessionId,
        message: IncomingMessage,
    ) -> AcpForwardResult {
        let (respond_tx, respond_rx) = oneshot::channel();

        let cmd = AcpCommand::ExecuteForBridge {
            session_id: acp_session_id.0.clone(),
            message,
            mode: ExecutionMode::Session,
            respond_to: respond_tx,
        };

        if let Err(e) = self.acp_tx.send(cmd).await {
            return AcpForwardResult::Failed(format!("ACP send failed: {}", e));
        }

        match tokio::time::timeout(std::time::Duration::from_secs(30), respond_rx).await {
            Ok(Ok(Ok(response))) => AcpForwardResult::Completed(Box::new(response)),
            Ok(Ok(Err(e))) => AcpForwardResult::Failed(format!("ACP execution failed: {}", e)),
            Ok(Err(_)) => AcpForwardResult::Failed("ACP response channel closed".to_string()),
            Err(_) => AcpForwardResult::Failed("ACP execution timed out".to_string()),
        }
    }

    /// Get the ACP session ID bound to a channel conversation.
    pub async fn get_binding(&self, channel_conversation_id: &str) -> Option<ChannelAcpBinding> {
        let bindings = self.bindings.read().await;
        bindings.get(channel_conversation_id).cloned()
    }

    /// Find the channel conversation for an ACP session.
    pub async fn get_channel_conversation(&self, acp_session_id: &str) -> Option<String> {
        let session_map = self.session_to_channel.read().await;
        session_map.get(acp_session_id).cloned()
    }

    /// Remove a binding.
    pub async fn remove_binding(&self, channel_conversation_id: &str) {
        let mut bindings = self.bindings.write().await;
        if let Some(binding) = bindings.remove(channel_conversation_id) {
            let mut session_map = self.session_to_channel.write().await;
            session_map.remove(&binding.acp_session_id.0);
            info!("Removed ACP binding for channel conversation {}", channel_conversation_id);
        }
    }

    /// Remove binding by ACP session ID.
    ///
    /// Acquires locks in the same order as `remove_binding` (bindings first,
    /// then session_to_channel) to avoid ABBA deadlock.
    pub async fn remove_by_session(&self, acp_session_id: &str) {
        // Peek at session_to_channel to find the conv_id without holding locks
        let conv_id = {
            let session_map = self.session_to_channel.read().await;
            session_map.get(acp_session_id).cloned()
        };

        let Some(conv_id) = conv_id else {
            return;
        };

        // Acquire locks in the same order as remove_binding
        let mut bindings = self.bindings.write().await;
        bindings.remove(&conv_id);
        let mut session_map = self.session_to_channel.write().await;
        session_map.remove(acp_session_id);
        info!("Removed ACP binding for session {}", acp_session_id);
    }

    /// List all active bindings.
    pub async fn list_bindings(&self) -> Vec<ChannelAcpBinding> {
        let bindings = self.bindings.read().await;
        bindings.values().cloned().collect()
    }

    /// Count active bindings.
    pub async fn binding_count(&self) -> usize {
        let bindings = self.bindings.read().await;
        bindings.len()
    }

    /// Pause an ACP session from a channel conversation.
    pub async fn pause_session(&self, channel_conversation_id: &str) -> Result<(), String> {
        let bindings = self.bindings.read().await;
        let binding = bindings
            .get(channel_conversation_id)
            .ok_or_else(|| "No ACP binding found for this conversation".to_string())?;

        let cmd = AcpCommand::Pause {
            session_id: binding.acp_session_id.0.clone(),
        };

        self.acp_tx
            .send(cmd)
            .await
            .map_err(|e| format!("ACP send failed: {}", e))
    }

    /// Resume an ACP session from a channel conversation.
    pub async fn resume_session(&self, channel_conversation_id: &str) -> Result<(), String> {
        let bindings = self.bindings.read().await;
        let binding = bindings
            .get(channel_conversation_id)
            .ok_or_else(|| "No ACP binding found for this conversation".to_string())?;

        let cmd = AcpCommand::Resume {
            session_id: binding.acp_session_id.0.clone(),
        };

        self.acp_tx
            .send(cmd)
            .await
            .map_err(|e| format!("ACP send failed: {}", e))
    }

    /// Cancel an ACP session from a channel conversation.
    pub async fn cancel_session(&self, channel_conversation_id: &str) -> Result<(), String> {
        let bindings = self.bindings.read().await;
        let binding = bindings
            .get(channel_conversation_id)
            .ok_or_else(|| "No ACP binding found for this conversation".to_string())?;

        let cmd = AcpCommand::Cancel {
            session_id: binding.acp_session_id.0.clone(),
        };

        self.acp_tx
            .send(cmd)
            .await
            .map_err(|e| format!("ACP send failed: {}", e))
    }

    /// Get the status of an ACP session bound to a channel conversation.
    pub async fn get_session_status(
        &self,
        channel_conversation_id: &str,
    ) -> Result<Option<AcpSessionStatus>, String> {
        let bindings = self.bindings.read().await;
        let binding = bindings
            .get(channel_conversation_id)
            .ok_or_else(|| "No ACP binding found".to_string())?;

        let (respond_tx, respond_rx) = oneshot::channel();
        let cmd = AcpCommand::GetStatus {
            session_id: binding.acp_session_id.0.clone(),
            respond_to: respond_tx,
        };

        self.acp_tx
            .send(cmd)
            .await
            .map_err(|e| format!("ACP send failed: {}", e))?;

        match tokio::time::timeout(std::time::Duration::from_secs(10), respond_rx).await {
            Ok(Ok(status)) => Ok(status),
            Ok(Err(_)) => Err("ACP response channel closed".to_string()),
            Err(_) => Err("ACP status request timed out".to_string()),
        }
    }

    /// Check if a channel conversation has an active ACP binding.
    pub async fn has_binding(&self, channel_conversation_id: &str) -> bool {
        let bindings = self.bindings.read().await;
        bindings.contains_key(channel_conversation_id)
    }
}

/// Parse ACP-related commands from a message.
pub fn parse_acp_command(content: &str) -> Option<AcpCommandRequest> {
    let trimmed = content.trim();

    // /spawn <agent_id> [instructions]
    if let Some(rest) = trimmed.strip_prefix("/spawn ") {
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        let agent_id = parts[0].to_string();
        let instructions = parts.get(1).map(|s| s.to_string());
        return Some(AcpCommandRequest::Spawn {
            agent_id,
            instructions,
            mode: ExecutionMode::Session,
        });
    }

    // /acp run <instructions>
    if let Some(instructions) = trimmed.strip_prefix("/acp run ") {
        return Some(AcpCommandRequest::Spawn {
            agent_id: "default".to_string(),
            instructions: Some(instructions.to_string()),
            mode: ExecutionMode::Run,
        });
    }

    // /acp pause
    if trimmed == "/acp pause" || trimmed == "/acp_pause" {
        return Some(AcpCommandRequest::Pause);
    }

    // /acp resume
    if trimmed == "/acp resume" || trimmed == "/acp_resume" {
        return Some(AcpCommandRequest::Resume);
    }

    // /acp cancel
    if trimmed == "/acp cancel" || trimmed == "/acp_cancel" {
        return Some(AcpCommandRequest::Cancel);
    }

    // /acp status
    if trimmed == "/acp status" || trimmed == "/acp_status" {
        return Some(AcpCommandRequest::Status);
    }

    None
}

/// Parsed ACP command request from a channel message.
#[derive(Debug, Clone)]
pub enum AcpCommandRequest {
    /// Spawn a new subagent.
    Spawn {
        agent_id: String,
        instructions: Option<String>,
        mode: ExecutionMode,
    },
    /// Pause the current ACP session.
    Pause,
    /// Resume the current ACP session.
    Resume,
    /// Cancel the current ACP session.
    Cancel,
    /// Get the status of the current ACP session.
    Status,
}

impl AcpCommandRequest {
    /// Returns true if this is a lifecycle command
    /// (pause/resume/cancel/status).
    pub fn is_lifecycle(&self) -> bool {
        matches!(
            self,
            AcpCommandRequest::Pause
                | AcpCommandRequest::Resume
                | AcpCommandRequest::Cancel
                | AcpCommandRequest::Status
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_bridge() -> (ChannelAcpBridge, mpsc::Receiver<AcpCommand>) {
        let (tx, rx) = mpsc::channel(100);
        (ChannelAcpBridge::new(tx), rx)
    }

    #[tokio::test]
    async fn test_binding_lifecycle() {
        let (bridge, _rx) = make_test_bridge();

        // Create a binding via forward_message
        let msg = IncomingMessage::new("user1", "conv1", "hello");
        let result = bridge
            .forward_message(msg, ExecutionMode::Session, "telegram")
            .await;

        // Without a real ACP actor to consume the command, the oneshot
        // response will time out and the bridge reports failure.
        assert!(matches!(result, AcpForwardResult::Failed(_)));

        // Check binding exists
        assert!(bridge.has_binding("conv1").await);
        assert_eq!(bridge.binding_count().await, 1);

        // Get binding
        let binding = bridge.get_binding("conv1").await;
        assert!(binding.is_some());
        assert_eq!(binding.unwrap().channel_name, "telegram");

        // Remove binding
        bridge.remove_binding("conv1").await;
        assert!(!bridge.has_binding("conv1").await);
    }

    #[tokio::test]
    async fn test_forward_with_existing_binding() {
        let (bridge, _rx) = make_test_bridge();

        // First message creates binding
        let msg1 = IncomingMessage::new("user1", "conv1", "first");
        let _ = bridge
            .forward_message(msg1, ExecutionMode::Session, "telegram")
            .await;

        // Second message reuses binding
        let msg2 = IncomingMessage::new("user1", "conv1", "second");
        let _ = bridge
            .forward_message(msg2, ExecutionMode::Session, "telegram")
            .await;

        // Should still be 1 binding
        assert_eq!(bridge.binding_count().await, 1);
    }

    #[tokio::test]
    async fn test_remove_by_session() {
        let (bridge, _rx) = make_test_bridge();

        let msg = IncomingMessage::new("user1", "conv1", "hello");
        let _ = bridge
            .forward_message(msg, ExecutionMode::Session, "telegram")
            .await;

        let binding = bridge.get_binding("conv1").await.unwrap();
        let acp_id = binding.acp_session_id.0.clone();

        bridge.remove_by_session(&acp_id).await;
        assert!(!bridge.has_binding("conv1").await);
    }

    #[tokio::test]
    async fn test_list_bindings() {
        let (bridge, _rx) = make_test_bridge();

        let msg1 = IncomingMessage::new("user1", "conv1", "hello");
        let msg2 = IncomingMessage::new("user2", "conv2", "world");
        let _ = bridge
            .forward_message(msg1, ExecutionMode::Session, "telegram")
            .await;
        let _ = bridge
            .forward_message(msg2, ExecutionMode::Session, "discord")
            .await;

        let list = bridge.list_bindings().await;
        assert_eq!(list.len(), 2);

        let channels: Vec<&str> = list.iter().map(|b| b.channel_name.as_str()).collect();
        assert!(channels.contains(&"telegram"));
        assert!(channels.contains(&"discord"));
    }

    #[tokio::test]
    async fn test_send_to_nonexistent_session() {
        let (tx, _rx) = mpsc::channel(100);
        let bridge = ChannelAcpBridge::new(tx);

        let msg = IncomingMessage::new("user1", "conv1", "hello");
        let result = bridge.send_to_session(&AcpSessionId::new(), msg).await;

        // Without a receiver consuming commands, the ExecuteForBridge message
        // will be buffered in the channel and the respond oneshot will time
        // out, resulting in a failure.
        assert!(matches!(result, AcpForwardResult::Failed(_)));
    }

    #[tokio::test]
    async fn test_pause_resume_cancel() {
        let (bridge, mut rx) = make_test_bridge();

        let msg = IncomingMessage::new("user1", "conv1", "hello");
        let _ = bridge
            .forward_message(msg, ExecutionMode::Session, "telegram")
            .await;

        // Drain the forward message command
        let _ = rx.recv().await;

        // Test pause
        let result = bridge.pause_session("conv1").await;
        assert!(result.is_ok());
        let received = rx.recv().await;
        assert!(matches!(received, Some(AcpCommand::Pause { .. })));

        // Test resume
        let result = bridge.resume_session("conv1").await;
        assert!(result.is_ok());
        let received = rx.recv().await;
        assert!(matches!(received, Some(AcpCommand::Resume { .. })));

        // Test cancel
        let result = bridge.cancel_session("conv1").await;
        assert!(result.is_ok());
        let received = rx.recv().await;
        assert!(matches!(received, Some(AcpCommand::Cancel { .. })));
    }

    #[tokio::test]
    async fn test_ops_on_missing_binding() {
        let (bridge, _rx) = make_test_bridge();

        assert!(bridge.pause_session("nonexistent").await.is_err());
        assert!(bridge.resume_session("nonexistent").await.is_err());
        assert!(bridge.cancel_session("nonexistent").await.is_err());
        assert!(bridge.get_session_status("nonexistent").await.is_err());
    }

    #[tokio::test]
    async fn test_get_channel_conversation() {
        let (bridge, _rx) = make_test_bridge();

        let msg = IncomingMessage::new("user1", "conv1", "hello");
        let _ = bridge
            .forward_message(msg, ExecutionMode::Session, "telegram")
            .await;

        let binding = bridge.get_binding("conv1").await.unwrap();
        let channel_conv = bridge
            .get_channel_conversation(&binding.acp_session_id.0)
            .await;
        assert_eq!(channel_conv, Some("conv1".to_string()));
    }

    #[test]
    fn test_parse_spawn_command() {
        let result = parse_acp_command("/spawn coder write a script");
        assert!(matches!(
            result,
            Some(AcpCommandRequest::Spawn { agent_id, instructions: Some(_), .. })
                if agent_id == "coder"
        ));

        // Just /spawn with no args
        assert!(parse_acp_command("/spawn").is_none());
    }

    #[test]
    fn test_parse_acp_run() {
        let result = parse_acp_command("/acp run do something");
        assert!(result.is_some());
        assert!(result.unwrap().is_lifecycle() == false);
    }

    #[test]
    fn test_parse_lifecycle_commands() {
        assert!(matches!(parse_acp_command("/acp pause"), Some(AcpCommandRequest::Pause)));
        assert!(matches!(parse_acp_command("/acp resume"), Some(AcpCommandRequest::Resume)));
        assert!(matches!(parse_acp_command("/acp cancel"), Some(AcpCommandRequest::Cancel)));
        assert!(matches!(parse_acp_command("/acp status"), Some(AcpCommandRequest::Status)));
    }

    #[test]
    fn test_parse_acp_pause_alt() {
        assert!(matches!(parse_acp_command("/acp_pause"), Some(AcpCommandRequest::Pause)));
    }

    #[test]
    fn test_parse_no_match() {
        assert!(parse_acp_command("hello world").is_none());
        assert!(parse_acp_command("/help").is_none());
    }

    #[test]
    fn test_acp_command_request_is_lifecycle() {
        assert!(AcpCommandRequest::Pause.is_lifecycle());
        assert!(AcpCommandRequest::Resume.is_lifecycle());
        assert!(AcpCommandRequest::Cancel.is_lifecycle());
        assert!(AcpCommandRequest::Status.is_lifecycle());
        assert!(!AcpCommandRequest::Spawn {
            agent_id: "default".to_string(),
            instructions: None,
            mode: ExecutionMode::Session,
        }
        .is_lifecycle());
    }
}
