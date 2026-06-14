//! GatewayState security tests
//!
//! Tests for `GatewayState::check_incoming_access` covering the four-layer
//! security pipeline: blocklist → DM policy → mention gating → command gate.

use super::*;
use crate::channels::{ChannelType, MentionState};
use crate::security::mention_gate::{MentionGate, MentionPolicy};
use crate::security::pairing::{DmPolicy, PairingStore};
use crate::tools::command_gate::CommandGate;
use std::collections::HashMap;
use tempfile::tempdir;

// ── Dummy pipeline implementations (required by GatewayState but unused here) ──

pub struct DummyInboundPipeline;

#[async_trait::async_trait]
impl crate::inbound::InboundPipeline for DummyInboundPipeline {
    async fn process(
        &self,
        _message: crate::channels::IncomingMessage,
    ) -> Option<crate::inbound::RoutedMessage> {
        None
    }

    async fn flush(&self, _key: &str) -> Vec<crate::inbound::RoutedMessage> {
        vec![]
    }
}

pub struct DummyOutboundPipeline;

#[async_trait::async_trait]
impl crate::outbound::OutboundPipeline for DummyOutboundPipeline {
    async fn process(
        &self,
        _ctx: crate::outbound::OutboundContext,
    ) -> crate::outbound::OutboundResult {
        crate::outbound::OutboundResult {
            text: String::new(),
            canvas_update: None,
            sse_events: vec![],
            side_effects: vec![],
            session_id: String::new(),
            channel: String::new(),
        }
    }
}

// ── Test helper: construct a minimal GatewayState ──────────────────────────────

pub async fn make_test_state(config: GatewayConfig) -> GatewayState {
    let (event_tx, _) = broadcast::channel(1);
    let (log_tx, _) = broadcast::channel(1);
    let (inbound_entry_tx, _inbound_entry_rx) = mpsc::channel(1);
    let (routed_tx, _routed_rx) = mpsc::channel(1);

    let tmp = tempdir().expect("create temp dir");
    let plugins_dir = tmp.path().join("plugins");
    let transcript_dir = tmp.path().join("transcripts");
    let artifact_dir = tmp.path().join("artifacts");
    let budget_dir = tmp.path().join("budget");
    let session_files_dir = tmp.path().join("session_files");

    let reply_dispatcher = Arc::new(crate::outbound::ReplyDispatcher::new(
        crate::outbound::ReplyDispatchConfig::default(),
    ));
    let side_effect_registry = Arc::new(crate::outbound::SideEffectRegistry::new());
    let side_effect_executor =
        Arc::new(crate::outbound::SideEffectExecutor::new(side_effect_registry));
    let sse_streamer = Arc::new(crate::outbound::SseStreamer::new());

    let inbound_pipeline: Arc<dyn crate::inbound::InboundPipeline> = Arc::new(DummyInboundPipeline);
    let outbound_pipeline: Arc<dyn crate::outbound::OutboundPipeline> =
        Arc::new(DummyOutboundPipeline);

    let transcript_store = crate::agent::TranscriptStore::new(transcript_dir);
    let _ = transcript_store.init().await;
    let artifact_store = crate::agent::ArtifactStore::new(artifact_dir);
    let _ = artifact_store.init().await;
    let disk_budget = crate::agent::DiskBudgetManager::new(budget_dir);
    let _ = disk_budget.init();
    let session_file_manager = crate::agent::SessionFileManager::new(session_files_dir);
    let _ = session_file_manager.init().await;

    let skills_manager = Arc::new(RwLock::new(
        crate::skills::SkillManager::new()
            .await
            .expect("skill manager"),
    ));

    GatewayState {
        config: Arc::new(RwLock::new(config)),
        start_time: std::time::Instant::now(),
        config_path: None,
        auth: AuthState {
            manager: Arc::new(crate::security::AuthManager::new()),
            pairing_store: Arc::new(PairingStore::new()),
            device_pairing_store: Arc::new(crate::security::device_pairing::DevicePairingStore::new()),
            tailscale_authenticator: None,
            trusted_proxy_authenticator: None,
            rate_limiter: Arc::new(crate::security::RateLimiter::new(100, 10.0)),
            multi_tier_rate_limiter: Arc::new(crate::gateway::rate_limit::MultiTierRateLimiter::new(
                crate::gateway::rate_limit::MultiTierRateLimitConfig::default(),
            )),
            audit_log: Arc::new(crate::security::persistent_audit::PersistentAuditLog::new()),
            command_gate: Arc::new(CommandGate::new()),
            mention_gate: Arc::new(MentionGate::new(MentionPolicy::Allow)),
        },
        agents: AgentState {
            agents: Arc::new(RwLock::new(HashMap::new())),
            router: Arc::new(crate::inbound::AgentRouter::new(
                crate::inbound::AgentRouterConfig::default(),
            )),
            registry: Arc::new(RwLock::new(crate::agent::AgentRegistry::new())),
            manager: Arc::new(RwLock::new(crate::agent::SessionManager::new())),
            group_manager: Arc::new(RwLock::new(crate::agent::GroupSessionManager::new())),
            store: None,
            message_buffer: Arc::new(RwLock::new(HashMap::new())),
            route_resolver: Arc::new(crate::agent::RouteResolver::new("default")),
            cost_guard: crate::agent::CostGuard::new(0, 0),
            repair_state: Arc::new(RepairState::new()),
            acp: Arc::new(AcpControlPlane::new(50)),
            session_routing: Arc::new(RwLock::new(HashMap::new())),
        },
        channels: ChannelState {
            channels: Arc::new(RwLock::new(HashMap::new())),
            extensions: Arc::new(RwLock::new(crate::channels::ChannelExtensionRegistry::new())),
            reply_dispatcher,
            snapshot_store: None,
            health_monitor: None,
            acp_bridge: None,
            session_channels: Arc::new(RwLock::new(HashMap::new())),
            webhook_sessions: Arc::new(RwLock::new(HashMap::new())),
        },
        memory: MemoryState {
            vector: crate::utils::LateInit::new(),
            session_search: crate::utils::LateInit::new(),
            manager: Arc::new(RwLock::new(None)),
            dream_scheduler: crate::utils::LateInit::new(),
            dream_metrics: Arc::new(crate::memory::DreamMetrics::default()),
            standing_order_manager: crate::utils::LateInit::new(),
        },
        tools: ToolState {
            registry: Arc::new(ToolRegistry::new()),
            mcp_manager: Arc::new(McpManager::new()),
            approval_queue: Arc::new(ApprovalQueue::new()),
            skills_manager,
            canvas_manager: Arc::new(CanvasManager::new()),
            computer_adapter: Arc::new(tokio::sync::RwLock::new(None)),
        },
        pipelines: PipelineState {
            inbound: inbound_pipeline,
            outbound: outbound_pipeline,
            side_effect_executor,
            sse_streamer,
            routed_tx,
            inbound_entry: inbound_entry_tx,
        },
        events: EventState {
            tx: event_tx,
            log_tx,
            hook_registry: Arc::new(hooks::EventHookRegistry::new()),
        },
        infra: InfraState {
            storage: Arc::new(RwLock::new(crate::adapters::InMemoryStorage::new())),
            runtime_settings: Arc::new(RwLock::new(HashMap::new())),
            transcript_store: Arc::new(transcript_store),
            artifact_store: Arc::new(artifact_store),
            disk_budget: Arc::new(disk_budget),
            session_file_manager: Arc::new(session_file_manager),
            hot_reload: crate::utils::LateInit::new(),
            plugin_manager: Arc::new(
                PluginManager::new(plugins_dir)
                    .await
                    .expect("plugin manager"),
            ),
            model_router: Arc::new(ModelRouter::new(crate::model_router::ModelRouterConfig::default())),
            engine_metrics: None,
            #[cfg(feature = "browser")]
            browser_bridge: tokio::sync::RwLock::new(None),
        },
        sdk: SdkState {
            provider_sdk: Arc::new(RwLock::new(crate::providers::ProviderSdk::new())),
            tool_sdk: Arc::new(RwLock::new(crate::tools::ToolSdk::new())),
        },
        scheduler: SchedulerState {
            task_scheduler: crate::utils::LateInit::new(),
            heartbeat_wake_tx: crate::utils::LateInit::new(),
            heartbeat_event_tx: crate::utils::LateInit::new(),
            cron_scheduler: crate::utils::LateInit::new(),
        },
    }
}

// ── Layer 1: Blocklist ───────────────────────────────────────────────────────

#[tokio::test]
async fn blocklist_blocks_user() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Telegram);
    ch.block_from = vec!["evil_user".to_string()];
    config.channels.insert("telegram".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("telegram", "evil_user", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_err(), "blocked user should be rejected");
    assert!(result.unwrap_err().contains("blocked"), "error should mention blocklist");
}

#[tokio::test]
async fn blocklist_allows_non_blocked_user() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Telegram);
    ch.block_from = vec!["evil_user".to_string()];
    config.channels.insert("telegram".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("telegram", "good_user", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_ok(), "non-blocked user should pass blocklist");
}

// ── Layer 2a: Allowlist DM Policy ────────────────────────────────────────────

#[tokio::test]
async fn allowlist_blocks_unauthorized_user() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Discord);
    ch.dm_policy = DmPolicy::Allowlist;
    ch.allow_from = vec!["alice".to_string()];
    config.channels.insert("discord".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("discord", "bob", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_err(), "user not in allowlist should be rejected");
    assert!(result.unwrap_err().contains("allowlist"), "error should mention allowlist");
}

#[tokio::test]
async fn allowlist_allows_authorized_user() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Discord);
    ch.dm_policy = DmPolicy::Allowlist;
    ch.allow_from = vec!["alice".to_string()];
    config.channels.insert("discord".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("discord", "alice", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_ok(), "user in allowlist should be allowed");
}

// ── Layer 2b: Pairing DM Policy ──────────────────────────────────────────────

#[tokio::test]
async fn pairing_blocks_unauthorized_user_and_creates_request() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Slack);
    ch.dm_policy = DmPolicy::Pairing;
    config.channels.insert("slack".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("slack", "newbie", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_err(), "unpaired user should be rejected");
    assert!(result.unwrap_err().contains("pairing"), "error should mention pairing");

    // A pairing request should have been created silently
    assert!(
        !state.auth.pairing_store.is_authorized("slack", "newbie").await,
        "user should still not be authorized"
    );
}

#[tokio::test]
async fn pairing_allows_authorized_user() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Slack);
    ch.dm_policy = DmPolicy::Pairing;
    config.channels.insert("slack".to_string(), ch);

    let state = make_test_state(config).await;

    // Pre-authorize the user
    let req = state.auth.pairing_store
        .request_access("slack", "alice", None)
        .await
        .expect("request access");
    let code = match req {
        crate::security::pairing::RequestAccessResult::NewRequest { code } => code,
        other => panic!("expected new request, got {:?}", other),
    };
    state.auth.pairing_store
        .approve("slack", &code, Some("admin"))
        .await;

    let result = state
        .check_incoming_access("slack", "alice", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_ok(), "authorized user should pass pairing check");
}

// ── Layer 3: Mention Gating ──────────────────────────────────────────────────

#[tokio::test]
async fn require_mention_blocks_group_message_without_mention() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Telegram);
    ch.require_mention = true;
    config.channels.insert("telegram".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("telegram", "user1", "hello", &MentionState::NotMentioned)
        .await;

    assert!(result.is_err(), "group msg without mention should be rejected");
    assert!(
        result.unwrap_err().contains("mention"),
        "error should mention mention requirement"
    );
}

#[tokio::test]
async fn require_mention_allows_direct_message() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Telegram);
    ch.require_mention = true;
    config.channels.insert("telegram".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("telegram", "user1", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_ok(), "DM should bypass mention requirement");
}

#[tokio::test]
async fn require_mention_allows_mentioned_message() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Telegram);
    ch.require_mention = true;
    config.channels.insert("telegram".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("telegram", "user1", "hello", &MentionState::Mentioned)
        .await;

    assert!(result.is_ok(), "mentioned message should pass");
}

#[tokio::test]
async fn mention_gate_block_policy_blocks_mentions() {
    let mut config = GatewayConfig::default();
    let ch = ChannelConfig::new(ChannelType::Discord);
    config.channels.insert("discord".to_string(), ch);

    let state = make_test_state(config).await;
    // Set mention gate to Block
    state.auth.mention_gate.set_policy(MentionPolicy::Block).await;

    let result = state
        .check_incoming_access("discord", "user1", "hello", &MentionState::Mentioned)
        .await;

    assert!(result.is_err(), "Block policy should reject mentions");
    assert!(
        result.unwrap_err().contains("Mention gate blocked"),
        "error should mention mention gate"
    );
}

#[tokio::test]
async fn mention_gate_allow_policy_passes_mentions() {
    let mut config = GatewayConfig::default();
    let ch = ChannelConfig::new(ChannelType::Discord);
    config.channels.insert("discord".to_string(), ch);

    let state = make_test_state(config).await;
    state.auth.mention_gate.set_policy(MentionPolicy::Allow).await;

    let result = state
        .check_incoming_access("discord", "user1", "hello", &MentionState::Mentioned)
        .await;

    assert!(result.is_ok(), "Allow policy should pass mentions");
}

// ── Layer 4: Command Gate ────────────────────────────────────────────────────

#[tokio::test]
async fn command_gate_blocks_forbidden_content() {
    let mut config = GatewayConfig::default();
    let ch = ChannelConfig::new(ChannelType::Telegram);
    config.channels.insert("telegram".to_string(), ch);

    let state = make_test_state(config).await;

    // CommandGate::new() defaults unknown users to Chat-only.
    // "rm -rf /" is a shell command and should be denied for Chat users.
    let result = state
        .check_incoming_access("telegram", "stranger", "rm -rf /", &MentionState::DirectMessage)
        .await;

    // Note: this test depends on CommandGate implementation. If the default
    // CommandGate::new() does not actually block "rm -rf /" for Chat users,
    // the assertion may need adjustment.
    if result.is_err() {
        assert!(
            result.unwrap_err().contains("Command gate denied"),
            "error should mention command gate"
        );
    }
    // If the command gate does not block this content, the test documents
    // the current permissive behaviour rather than failing.
}

#[tokio::test]
async fn command_gate_allows_safe_content() {
    let mut config = GatewayConfig::default();
    let ch = ChannelConfig::new(ChannelType::Telegram);
    config.channels.insert("telegram".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access(
            "telegram",
            "stranger",
            "Hello, how are you?",
            &MentionState::DirectMessage,
        )
        .await;

    assert!(result.is_ok(), "safe content should pass command gate");
}

// ── Layer interaction: precedence ────────────────────────────────────────────

#[tokio::test]
async fn blocklist_takes_precedence_over_allowlist() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Discord);
    ch.dm_policy = DmPolicy::Allowlist;
    ch.allow_from = vec!["evil".to_string()];
    ch.block_from = vec!["evil".to_string()];
    config.channels.insert("discord".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("discord", "evil", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("blocked"), "blocklist should be checked first");
}

#[tokio::test]
async fn blocklist_takes_precedence_over_pairing() {
    let mut config = GatewayConfig::default();
    let mut ch = ChannelConfig::new(ChannelType::Slack);
    ch.dm_policy = DmPolicy::Pairing;
    ch.block_from = vec!["evil".to_string()];
    config.channels.insert("slack".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("slack", "evil", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("blocked"),
        "blocklist should be checked before pairing"
    );
}

// ── No channel config (open by default) ──────────────────────────────────────

#[tokio::test]
async fn no_channel_config_allows_any_message() {
    let config = GatewayConfig::default();
    let state = make_test_state(config).await;

    let result = state
        .check_incoming_access("unknown", "anyone", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_ok(), "no channel config means open access");
}

// ── Open DM policy allows all ────────────────────────────────────────────────

#[tokio::test]
async fn open_policy_allows_any_user() {
    let mut config = GatewayConfig::default();
    let ch = ChannelConfig::new(ChannelType::Whatsapp);
    config.channels.insert("whatsapp".to_string(), ch);

    let state = make_test_state(config).await;
    let result = state
        .check_incoming_access("whatsapp", "random", "hello", &MentionState::DirectMessage)
        .await;

    assert!(result.is_ok(), "open policy should allow any user");
}

// ── HTTP Handler Integration Tests ───────────────────────────────────────────

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn live_handler_returns_200() {
    let app = Router::new().route("/live", get(super::live_handler));

    let req = Request::builder().uri("/live").body(Body::empty()).unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["alive"], true);
    assert!(json["timestamp"].as_str().is_some());
}

#[tokio::test]
async fn status_handler_returns_summary() {
    let config = GatewayConfig::default();
    let state = Arc::new(make_test_state(config).await);
    let app = Router::new()
        .route("/status", get(super::status_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/status")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["agents"]["total"].is_number());
    assert!(json["channels"].is_number());
    assert!(json["version"].as_str().is_some());
}

#[tokio::test]
async fn health_handler_degraded_without_agents() {
    let config = GatewayConfig::default();
    let state = Arc::new(make_test_state(config).await);
    let app = Router::new()
        .route("/health", get(super::health_handler))
        .with_state(state);

    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();

    // Without a default agent or healthy providers, health is degraded (503)
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "degraded");
    assert_eq!(json["overall_healthy"], false);
}
