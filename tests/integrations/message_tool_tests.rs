//! Message tool integration tests
//!
//! Tests MessageTool actions via a MockChannel to verify:
//! - Each action routes to the correct Channel method
//! - Capability checks reject unsupported actions
//! - Missing required arguments produce validation errors

use super::*;
use async_trait::async_trait;
use manta::channels::{Channel, ChannelCapabilities, ChatType, ConversationId, OutgoingMessage};
use manta::core::models::Id;
use manta::gateway::{GatewayConfig, GatewayState};
use manta::tools::message::MessageTool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

// ── Dummy pipelines (required by GatewayState but unused by these tests) ─────

struct DummyInboundPipeline;

#[async_trait]
impl manta::inbound::InboundPipeline for DummyInboundPipeline {
    async fn process(
        &self,
        _message: manta::channels::IncomingMessage,
    ) -> Option<manta::inbound::RoutedMessage> {
        None
    }

    async fn flush(&self, _key: &str) -> Vec<manta::inbound::RoutedMessage> {
        vec![]
    }
}

struct DummyOutboundPipeline;

#[async_trait]
impl manta::outbound::OutboundPipeline for DummyOutboundPipeline {
    async fn process(
        &self,
        _ctx: manta::outbound::OutboundContext,
    ) -> manta::outbound::OutboundResult {
        manta::outbound::OutboundResult {
            text: String::new(),
            canvas_update: None,
            sse_events: vec![],
            side_effects: vec![],
            session_id: String::new(),
            channel: String::new(),
        }
    }
}

// ── Test helper: construct a minimal GatewayState ─────────────────────────────

async fn make_test_state(config: GatewayConfig) -> GatewayState {
    let (event_tx, _) = broadcast::channel(1);
    let (message_queue_tx, _message_queue_rx) = mpsc::channel(1);
    let (routed_tx, _routed_rx) = mpsc::channel(1);

    let tmp = tempfile::tempdir().expect("create temp dir");
    let plugins_dir = tmp.path().join("plugins");
    let transcript_dir = tmp.path().join("transcripts");
    let artifact_dir = tmp.path().join("artifacts");
    let budget_dir = tmp.path().join("budget");
    let session_files_dir = tmp.path().join("session_files");

    let reply_dispatcher = Arc::new(manta::outbound::ReplyDispatcher::new(
        manta::outbound::ReplyDispatchConfig::default(),
    ));
    let side_effect_registry = Arc::new(manta::outbound::SideEffectRegistry::new());
    let side_effect_executor =
        Arc::new(manta::outbound::SideEffectExecutor::new(side_effect_registry));
    let sse_streamer = Arc::new(manta::outbound::SseStreamer::new());

    let inbound_pipeline: Arc<dyn manta::inbound::InboundPipeline> = Arc::new(DummyInboundPipeline);
    let outbound_pipeline: Arc<dyn manta::outbound::OutboundPipeline> =
        Arc::new(DummyOutboundPipeline);

    let transcript_store = manta::agent::TranscriptStore::new(transcript_dir);
    let _ = transcript_store.init();
    let artifact_store = manta::agent::ArtifactStore::new(artifact_dir);
    let _ = artifact_store.init();
    let disk_budget = manta::agent::DiskBudgetManager::new(budget_dir);
    let _ = disk_budget.init();
    let session_file_manager = manta::agent::SessionFileManager::new(session_files_dir);
    let _ = session_file_manager.init().await;

    GatewayState {
        config: Arc::new(RwLock::new(config)),
        channels: Arc::new(RwLock::new(HashMap::new())),
        agents: Arc::new(RwLock::new(HashMap::new())),
        session_routing: Arc::new(RwLock::new(HashMap::new())),
        agent_router: Arc::new(manta::inbound::AgentRouter::new(
            manta::inbound::AgentRouterConfig::default(),
        )),
        session_channels: Arc::new(RwLock::new(HashMap::new())),
        webhook_sessions: Arc::new(RwLock::new(HashMap::new())),
        model_router: Arc::new(manta::model_router::ModelRouter::new(
            manta::model_router::ModelRouterConfig::default(),
        )),
        tool_registry: Arc::new(manta::tools::ToolRegistry::new()),
        event_tx,
        hook_registry: Arc::new(manta::gateway::hooks::EventHookRegistry::new()),
        message_queue: message_queue_tx,
        canvas_manager: Arc::new(manta::canvas::CanvasManager::new()),
        plugin_manager: Arc::new(
            manta::plugins::PluginManager::new(plugins_dir)
                .await
                .expect("plugin manager"),
        ),
        acp: Arc::new(manta::acp::AcpControlPlane::new()),
        vector_memory: RwLock::new(None),
        session_search: RwLock::new(None),
        memory_manager: RwLock::new(None),
        hot_reload: RwLock::new(None),
        cron_scheduler: RwLock::new(None),
        auth_manager: Arc::new(manta::security::AuthManager::new()),
        pairing_store: Arc::new(manta::security::pairing::PairingStore::new()),
        device_pairing_store: Arc::new(manta::security::device_pairing::DevicePairingStore::new()),
        command_gate: Arc::new(manta::tools::command_gate::CommandGate::new()),
        mention_gate: Arc::new(manta::security::mention_gate::MentionGate::new(
            manta::security::mention_gate::MentionPolicy::Allow,
        )),
        audit_log: Arc::new(manta::security::persistent_audit::PersistentAuditLog::new()),
        rate_limiter: Arc::new(manta::security::RateLimiter::new(100, 10.0)),
        multi_tier_rate_limiter: Arc::new(manta::gateway::rate_limit::MultiTierRateLimiter::new(
            manta::gateway::rate_limit::MultiTierRateLimitConfig::default(),
        )),
        storage: Arc::new(RwLock::new(manta::adapters::InMemoryStorage::new())),
        skills_manager: Arc::new(RwLock::new(
            manta::skills::SkillManager::new()
                .await
                .expect("skill manager"),
        )),
        agent_registry: Arc::new(RwLock::new(manta::agent::AgentRegistry::new())),
        session_manager: Arc::new(RwLock::new(manta::agent::SessionManager::new())),
        session_store: None,
        mcp_manager: Arc::new(manta::tools::mcp::McpManager::new()),
        config_path: None,
        runtime_settings: Arc::new(RwLock::new(HashMap::new())),
        approval_queue: Arc::new(manta::tools::approval::ApprovalQueue::new()),
        repair_state: Arc::new(manta::gateway::RepairState::new()),
        cost_guard: manta::agent::CostGuard::new(0, 0),
        reply_dispatcher,
        routed_tx,
        inbound_pipeline,
        outbound_pipeline,
        side_effect_executor,
        sse_streamer,
        channel_extensions: Arc::new(RwLock::new(manta::channels::ChannelExtensionRegistry::new())),
        provider_sdk: Arc::new(RwLock::new(manta::providers::ProviderSdk::new())),
        tool_sdk: Arc::new(RwLock::new(manta::tools::ToolSdk::new())),
        session_message_buffer: Arc::new(RwLock::new(HashMap::new())),
        route_resolver: Arc::new(manta::agent::RouteResolver::new("default")),
        transcript_store: Arc::new(transcript_store),
        artifact_store: Arc::new(artifact_store),
        disk_budget: Arc::new(disk_budget),
        session_file_manager: Arc::new(session_file_manager),
        group_session_manager: Arc::new(RwLock::new(manta::agent::GroupSessionManager::new())),
        #[cfg(feature = "browser")]
        browser_bridge: tokio::sync::RwLock::new(None),
    }
}

// ── MockChannel ──────────────────────────────────────────────────────────────

/// A mock channel that records every method call for later verification.
#[derive(Debug, Clone)]
struct MockChannel {
    name: String,
    caps: ChannelCapabilities,
    calls: Arc<RwLock<Vec<String>>>,
}

impl MockChannel {
    fn new(name: impl Into<String>, caps: ChannelCapabilities) -> Self {
        Self {
            name: name.into(),
            caps,
            calls: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Drain recorded calls (for assertions).
    async fn drain_calls(&self) -> Vec<String> {
        let mut guard = self.calls.write().await;
        std::mem::take(&mut *guard)
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn capabilities(&self) -> ChannelCapabilities {
        self.caps.clone()
    }

    async fn start(&self) -> manta::Result<()> {
        self.calls.write().await.push("start".to_string());
        Ok(())
    }

    async fn stop(&self) -> manta::Result<()> {
        self.calls.write().await.push("stop".to_string());
        Ok(())
    }

    async fn send(&self, msg: OutgoingMessage) -> manta::Result<Id> {
        let reply = msg
            .reply_to
            .map(|r| format!(":reply_to={}", r))
            .unwrap_or_default();
        self.calls
            .write()
            .await
            .push(format!("send:{}:{}{}", msg.conversation_id, msg.content, reply));
        Ok(Id::new())
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> manta::Result<()> {
        self.calls
            .write()
            .await
            .push(format!("typing:{}", conversation_id));
        Ok(())
    }

    async fn edit_message(&self, message_id: Id, new_content: String) -> manta::Result<()> {
        self.calls
            .write()
            .await
            .push(format!("edit:{}:{}", message_id, new_content));
        Ok(())
    }

    async fn delete_message(&self, message_id: Id) -> manta::Result<()> {
        self.calls
            .write()
            .await
            .push(format!("delete:{}", message_id));
        Ok(())
    }

    async fn health_check(&self) -> manta::Result<bool> {
        Ok(true)
    }

    async fn add_reaction(&self, message_id: Id, emoji: String) -> manta::Result<()> {
        self.calls
            .write()
            .await
            .push(format!("react:{}:{}", message_id, emoji));
        Ok(())
    }

    async fn remove_reaction(&self, message_id: Id, emoji: String) -> manta::Result<()> {
        self.calls
            .write()
            .await
            .push(format!("unreact:{}:{}", message_id, emoji));
        Ok(())
    }

    async fn pin_message(&self, message_id: Id) -> manta::Result<()> {
        self.calls.write().await.push(format!("pin:{}", message_id));
        Ok(())
    }

    async fn unpin_message(&self, message_id: Id) -> manta::Result<()> {
        self.calls
            .write()
            .await
            .push(format!("unpin:{}", message_id));
        Ok(())
    }

    async fn create_thread(
        &self,
        message_id: Id,
        title: Option<String>,
    ) -> manta::Result<ConversationId> {
        self.calls
            .write()
            .await
            .push(format!("thread:{}:{:?}", message_id, title));
        Ok(ConversationId::new("thread-123"))
    }

    async fn send_poll(
        &self,
        conversation_id: ConversationId,
        question: String,
        options: Vec<String>,
    ) -> manta::Result<Id> {
        self.calls
            .write()
            .await
            .push(format!("poll:{}:{}:{:?}", conversation_id, question, options));
        Ok(Id::new())
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn full_caps() -> ChannelCapabilities {
    ChannelCapabilities {
        chat_types: vec![ChatType::Direct, ChatType::Group],
        supports_formatting: true,
        supports_attachments: true,
        supports_images: true,
        supports_threads: true,
        supports_typing: true,
        supports_buttons: true,
        supports_commands: true,
        supports_reactions: true,
        supports_edit: true,
        supports_unsend: true,
        supports_effects: false,
    }
}

fn restricted_caps() -> ChannelCapabilities {
    ChannelCapabilities {
        supports_edit: false,
        supports_unsend: false,
        supports_reactions: false,
        supports_threads: false,
        supports_typing: false,
        ..full_caps()
    }
}

async fn setup_with_channel(channel: Arc<dyn Channel>) -> (MessageTool, String) {
    let config = GatewayConfig::default();
    let state: GatewayState = make_test_state(config).await;
    state
        .channels
        .write()
        .await
        .insert("mock".to_string(), channel);
    let tool = MessageTool::new(Arc::new(state));
    (tool, "mock".to_string())
}

// ── Action routing tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn send_action_routes_to_channel_send() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;

    let result = tool
        .execute(
            json!({
                "action": "send",
                "channel": ch,
                "conversation_id": "conv1",
                "content": "hello world"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "send should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls, vec!["send:conv1:hello world"]);
}

#[tokio::test]
async fn reply_action_routes_to_channel_send_with_reply_to() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "reply",
                "channel": ch,
                "conversation_id": "conv1",
                "content": "reply text",
                "message_id": msg_id
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "reply should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("send:conv1:reply text:reply_to="));
}

#[tokio::test]
async fn edit_action_routes_to_channel_edit() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "edit",
                "channel": ch,
                "conversation_id": "conv1",
                "content": "edited",
                "message_id": msg_id
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "edit should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("edit:"));
    assert!(calls[0].ends_with(":edited"));
}

#[tokio::test]
async fn delete_action_routes_to_channel_delete() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "delete",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "delete should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("delete:"));
}

#[tokio::test]
async fn typing_action_routes_to_channel_send_typing() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;

    let result = tool
        .execute(
            json!({
                "action": "typing",
                "channel": ch,
                "conversation_id": "conv1"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "typing should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls, vec!["typing:conv1"]);
}

#[tokio::test]
async fn react_action_routes_to_channel_add_reaction() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "react",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id,
                "emoji": "👍"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "react should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("react:"));
    assert!(calls[0].ends_with(":👍"));
}

#[tokio::test]
async fn unreact_action_routes_to_channel_remove_reaction() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "unreact",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id,
                "emoji": "👍"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "unreact should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("unreact:"));
}

#[tokio::test]
async fn pin_action_routes_to_channel_pin() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "pin",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "pin should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("pin:"));
}

#[tokio::test]
async fn unpin_action_routes_to_channel_unpin() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "unpin",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "unpin should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("unpin:"));
}

#[tokio::test]
async fn thread_create_action_routes_to_channel_create_thread() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "thread_create",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id,
                "thread_title": "Discussion"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "thread_create should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("thread:"));
    assert!(calls[0].contains("Discussion"));
}

#[tokio::test]
async fn poll_action_routes_to_channel_send_poll() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;

    let result = tool
        .execute(
            json!({
                "action": "poll",
                "channel": ch,
                "conversation_id": "conv1",
                "poll_question": "Q?",
                "poll_options": ["A", "B"]
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(result.success, "poll should succeed: {:?}", result.error);
    let calls = mock.drain_calls().await;
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("poll:conv1:Q?:"));
}

// ── Capability rejection tests ───────────────────────────────────────────────

#[tokio::test]
async fn edit_rejected_when_channel_does_not_support_edit() {
    let mock = Arc::new(MockChannel::new("mock", restricted_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "edit",
                "channel": ch,
                "conversation_id": "conv1",
                "content": "edited",
                "message_id": msg_id
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success, "edit should be rejected");
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("does not support message editing"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty(), "no channel method should be called");
}

#[tokio::test]
async fn delete_rejected_when_channel_does_not_support_unsend() {
    let mock = Arc::new(MockChannel::new("mock", restricted_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "delete",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success, "delete should be rejected");
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("does not support message deletion"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}

#[tokio::test]
async fn typing_rejected_when_channel_does_not_support_typing() {
    let mock = Arc::new(MockChannel::new("mock", restricted_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;

    let result = tool
        .execute(
            json!({
                "action": "typing",
                "channel": ch,
                "conversation_id": "conv1"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success, "typing should be rejected");
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("does not support typing indicators"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}

#[tokio::test]
async fn react_rejected_when_channel_does_not_support_reactions() {
    let mock = Arc::new(MockChannel::new("mock", restricted_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "react",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id,
                "emoji": "👍"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success, "react should be rejected");
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("does not support reactions"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}

#[tokio::test]
async fn thread_create_rejected_when_channel_does_not_support_threads() {
    let mock = Arc::new(MockChannel::new("mock", restricted_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "thread_create",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success, "thread_create should be rejected");
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("does not support threads"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}

// ── Validation error tests ───────────────────────────────────────────────────

#[tokio::test]
async fn unknown_channel_returns_error() {
    let config = GatewayConfig::default();
    let state = Arc::new(make_test_state(config).await);
    let tool = MessageTool::new(state);

    let result = tool
        .execute(
            json!({
                "action": "send",
                "channel": "nonexistent",
                "conversation_id": "conv1",
                "content": "hello"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success);
    assert!(result.error.as_ref().unwrap().contains("not found"));
}

#[tokio::test]
async fn send_without_content_returns_error() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;

    let result = tool
        .execute(
            json!({
                "action": "send",
                "channel": ch,
                "conversation_id": "conv1"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success);
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("content is required"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}

#[tokio::test]
async fn reply_without_message_id_returns_error() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;

    let result = tool
        .execute(
            json!({
                "action": "reply",
                "channel": ch,
                "conversation_id": "conv1",
                "content": "reply"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success);
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("message_id is required"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}

#[tokio::test]
async fn react_without_emoji_returns_error() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;
    let msg_id = Id::new().to_string();

    let result = tool
        .execute(
            json!({
                "action": "react",
                "channel": ch,
                "conversation_id": "conv1",
                "message_id": msg_id
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success);
    assert!(result.error.as_ref().unwrap().contains("emoji is required"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}

#[tokio::test]
async fn poll_without_question_returns_error() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;

    let result = tool
        .execute(
            json!({
                "action": "poll",
                "channel": ch,
                "conversation_id": "conv1",
                "poll_options": ["A"]
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success);
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("poll_question is required"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}

#[tokio::test]
async fn poll_without_options_returns_error() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;

    let result = tool
        .execute(
            json!({
                "action": "poll",
                "channel": ch,
                "conversation_id": "conv1",
                "poll_question": "Q?"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success);
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("poll_options is required"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}

#[tokio::test]
async fn unknown_action_returns_error() {
    let mock = Arc::new(MockChannel::new("mock", full_caps()));
    let (tool, ch) = setup_with_channel(mock.clone()).await;

    let result = tool
        .execute(
            json!({
                "action": "dance",
                "channel": ch,
                "conversation_id": "conv1"
            }),
            &test_context(),
        )
        .await
        .expect("tool execute should not error");

    assert!(!result.success);
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("Unknown message action"));
    let calls = mock.drain_calls().await;
    assert!(calls.is_empty());
}
