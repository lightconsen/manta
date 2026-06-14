//! Domain-grouped sub-states for `GatewayState`.
//!
//! This module splits the monolithic flat gateway state into cohesive,
//! lifecycle-aligned sub-states (auth, agents, channels, memory, tools,
//! pipelines, events, infrastructure, sdk, scheduler). Each sub-state owns
//! the services that belong together, making initialization, shutdown, and
//! testing easier to reason about.
//!
//! During the migration window, `GatewayState` exposes deprecated forwarding
//! getters so existing call sites keep compiling while handlers are migrated
//! to nested access (`state.auth.manager` instead of `state.auth_manager`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc, Mutex, RwLock};

use crate::acp::AcpControlPlane;
use crate::agent::{
    session_store::SessionStore, AgentRegistry, ArtifactStore, CostGuard, DiskBudgetManager,
    GroupSessionManager, RouteResolver, SessionFileManager, SessionManager, TranscriptStore,
};
use crate::gateway::AgentHandle;
use crate::adapters::Storage;
use crate::canvas::CanvasManager;
use crate::channels::{
    Channel, ChannelAcpBridge, ChannelExtensionRegistry, ChannelHealthMonitor, IncomingMessage,
};
use crate::channels::snapshot::AccountSnapshotStore;
use crate::config::hot_reload::HotReloadManager;
use crate::cron::cron::CronScheduler;
use crate::gateway::hooks::EventHookRegistry;
use crate::gateway::rate_limit::MultiTierRateLimiter;
use crate::gateway::RepairState;
use crate::gateway::{GatewayConfig, GatewayEvent};
use crate::heartbeat::{HeartbeatEvent, WakeRequest};
use crate::inbound::{AgentRouter, InboundPipeline, RoutedMessage};
use crate::memory::vector::VectorMemoryService;
use crate::memory::{DreamMetrics, DreamScheduler, MemoryManager, SessionSearch};
use crate::model_router::ModelRouter;
use crate::outbound::{OutboundPipeline, ReplyDispatcher, SideEffectExecutor, SseStreamer};
use crate::planner::TaskScheduler;
use crate::plugins::PluginManager;
use crate::providers::ProviderSdk;
use crate::security::{
    mention_gate::MentionGate, persistent_audit::PersistentAuditLog, pairing::PairingStore,
    tailscale::TailscaleAuthenticator, trusted_proxy::TrustedProxyAuthenticator, AuthManager,
    RateLimiter,
};
use crate::security::device_pairing::DevicePairingStore;
use crate::skills::SkillManager;
use crate::tools::{
    approval::ApprovalQueue, command_gate::CommandGate, mcp::McpManager, ToolRegistry, ToolSdk,
};
use crate::utils::LateInit;

/// Authentication, authorization, and security-related state.
pub struct AuthState {
    pub manager: Arc<AuthManager>,
    pub pairing_store: Arc<PairingStore>,
    pub device_pairing_store: Arc<DevicePairingStore>,
    pub tailscale_authenticator: Option<Arc<TailscaleAuthenticator>>,
    pub trusted_proxy_authenticator: Option<Arc<TrustedProxyAuthenticator>>,
    pub rate_limiter: Arc<RateLimiter>,
    pub multi_tier_rate_limiter: Arc<MultiTierRateLimiter>,
    pub audit_log: Arc<PersistentAuditLog>,
    pub command_gate: Arc<CommandGate>,
    pub mention_gate: Arc<MentionGate>,
}

/// Agent runtime, routing, and session management state.
pub struct AgentState {
    pub agents: Arc<RwLock<HashMap<String, AgentHandle>>>,
    pub router: Arc<AgentRouter>,
    pub registry: Arc<RwLock<AgentRegistry>>,
    pub manager: Arc<RwLock<SessionManager>>,
    pub group_manager: Arc<RwLock<GroupSessionManager>>,
    pub store: Option<Arc<SessionStore>>,
    pub message_buffer: Arc<RwLock<HashMap<String, Vec<crate::gateway::BufferedMessage>>>>,
    pub route_resolver: Arc<RouteResolver>,
    pub cost_guard: Arc<CostGuard>,
    pub repair_state: Arc<RepairState>,
    pub acp: Arc<AcpControlPlane>,
    /// DEPRECATED: use `router` instead for new code.
    pub session_routing: Arc<RwLock<HashMap<String, String>>>,
}

/// Channel adapters, extensions, and response dispatch state.
pub struct ChannelState {
    pub channels: Arc<RwLock<HashMap<String, Arc<dyn Channel>>>>,
    pub extensions: Arc<RwLock<ChannelExtensionRegistry>>,
    pub reply_dispatcher: Arc<ReplyDispatcher>,
    pub snapshot_store: Option<AccountSnapshotStore>,
    pub health_monitor: Option<Arc<ChannelHealthMonitor>>,
    pub acp_bridge: Option<Arc<ChannelAcpBridge>>,
    /// Session to channel mapping: session_id -> (channel_name, channel_specific_id).
    pub session_channels: Arc<RwLock<HashMap<String, (String, String)>>>,
    /// Webhook session storage: platform_key -> session_uuid.
    pub webhook_sessions: Arc<RwLock<HashMap<String, String>>>,
}

/// Memory, search, and background consolidation state.
pub struct MemoryState {
    pub vector: LateInit<Arc<VectorMemoryService>>,
    pub session_search: LateInit<Arc<SessionSearch>>,
    pub manager: Arc<RwLock<Option<Arc<MemoryManager>>>>,
    pub dream_scheduler: LateInit<DreamScheduler>,
    pub dream_metrics: Arc<DreamMetrics>,
    pub standing_order_manager: LateInit<crate::standing_orders::StandingOrderManager>,
}

/// Tool registry, MCP, skills, and canvas state.
pub struct ToolState {
    pub registry: Arc<ToolRegistry>,
    pub mcp_manager: Arc<McpManager>,
    pub approval_queue: Arc<ApprovalQueue>,
    pub skills_manager: Arc<RwLock<SkillManager>>,
    pub canvas_manager: Arc<CanvasManager>,
    pub computer_adapter: Arc<RwLock<Option<Arc<dyn crate::computer::ComputerAdapter>>>>,
}

/// Message routing pipelines and streaming infrastructure state.
pub struct PipelineState {
    pub inbound: Arc<dyn InboundPipeline>,
    pub outbound: Arc<dyn OutboundPipeline>,
    pub side_effect_executor: Arc<SideEffectExecutor>,
    pub sse_streamer: Arc<SseStreamer>,
    pub routed_tx: mpsc::Sender<RoutedMessage>,
    pub inbound_entry: mpsc::Sender<IncomingMessage>,
}

/// Event broadcasting and hook registry state.
pub struct EventState {
    pub tx: broadcast::Sender<GatewayEvent>,
    pub log_tx: broadcast::Sender<String>,
    pub hook_registry: Arc<EventHookRegistry>,
}

/// Persistence, storage, plugins, and cross-cutting infrastructure state.
pub struct InfraState {
    pub storage: Arc<RwLock<dyn Storage>>,
    pub runtime_settings: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    pub transcript_store: Arc<TranscriptStore>,
    pub artifact_store: Arc<ArtifactStore>,
    pub disk_budget: Arc<DiskBudgetManager>,
    pub session_file_manager: Arc<SessionFileManager>,
    pub hot_reload: LateInit<Arc<HotReloadManager>>,
    pub plugin_manager: Arc<PluginManager>,
    pub model_router: Arc<ModelRouter>,
    /// Engine metrics counters (populated when a core `Engine` is wired in).
    pub engine_metrics: Option<Arc<crate::core::EngineMetrics>>,
    /// Browser bridge server (started when browser.bridge_enabled is true).
    #[cfg(feature = "browser")]
    pub browser_bridge: RwLock<Option<crate::browser::BrowserBridge>>,
}

/// Dynamic provider and tool SDK state.
pub struct SdkState {
    pub provider_sdk: Arc<RwLock<ProviderSdk>>,
    pub tool_sdk: Arc<RwLock<ToolSdk>>,
}

/// Background schedulers and heartbeat state.
pub struct SchedulerState {
    pub task_scheduler: LateInit<Arc<Mutex<TaskScheduler>>>,
    pub heartbeat_wake_tx: LateInit<mpsc::Sender<WakeRequest>>,
    pub heartbeat_event_tx: LateInit<broadcast::Sender<HeartbeatEvent>>,
    pub cron_scheduler: LateInit<Arc<Mutex<CronScheduler>>>,
}

/// Shared gateway state grouped by domain.
pub struct GatewayState {
    /// Configuration
    pub config: Arc<RwLock<GatewayConfig>>,
    /// Gateway startup time for uptime calculations
    pub start_time: Instant,
    /// Path to the config file (for runtime persistence)
    pub config_path: Option<PathBuf>,

    pub auth: AuthState,
    pub agents: AgentState,
    pub channels: ChannelState,
    pub memory: MemoryState,
    pub tools: ToolState,
    pub pipelines: PipelineState,
    pub events: EventState,
    pub infra: InfraState,
    pub sdk: SdkState,
    pub scheduler: SchedulerState,
}

impl GatewayState {
    // ── Deprecated forwarding getters (remove once call sites are migrated) ──

    /// Deprecated: use `self.auth.manager` instead.
    pub fn auth_manager(&self) -> Arc<AuthManager> {
        self.auth.manager.clone()
    }

    /// Deprecated: use `self.auth.pairing_store` instead.
    pub fn pairing_store(&self) -> Arc<PairingStore> {
        self.auth.pairing_store.clone()
    }

    /// Deprecated: use `self.auth.device_pairing_store` instead.
    pub fn device_pairing_store(&self) -> Arc<DevicePairingStore> {
        self.auth.device_pairing_store.clone()
    }

    /// Deprecated: use `self.auth.tailscale_authenticator` instead.
    pub fn tailscale_authenticator(&self) -> Option<Arc<TailscaleAuthenticator>> {
        self.auth.tailscale_authenticator.clone()
    }

    /// Deprecated: use `self.auth.trusted_proxy_authenticator` instead.
    pub fn trusted_proxy_authenticator(&self) -> Option<Arc<TrustedProxyAuthenticator>> {
        self.auth.trusted_proxy_authenticator.clone()
    }

    /// Deprecated: use `self.auth.rate_limiter` instead.
    pub fn rate_limiter(&self) -> Arc<RateLimiter> {
        self.auth.rate_limiter.clone()
    }

    /// Deprecated: use `self.auth.multi_tier_rate_limiter` instead.
    pub fn multi_tier_rate_limiter(&self) -> Arc<MultiTierRateLimiter> {
        self.auth.multi_tier_rate_limiter.clone()
    }

    /// Deprecated: use `self.auth.audit_log` instead.
    pub fn audit_log(&self) -> Arc<PersistentAuditLog> {
        self.auth.audit_log.clone()
    }

    /// Deprecated: use `self.auth.command_gate` instead.
    pub fn command_gate(&self) -> Arc<CommandGate> {
        self.auth.command_gate.clone()
    }

    /// Deprecated: use `self.auth.mention_gate` instead.
    pub fn mention_gate(&self) -> Arc<MentionGate> {
        self.auth.mention_gate.clone()
    }

    /// Deprecated: use `self.agents.agents` instead.
    pub fn agents(&self) -> Arc<RwLock<HashMap<String, AgentHandle>>> {
        self.agents.agents.clone()
    }

    /// Deprecated: use `self.agents.router` instead.
    pub fn agent_router(&self) -> Arc<AgentRouter> {
        self.agents.router.clone()
    }

    /// Deprecated: use `self.agents.registry` instead.
    pub fn agent_registry(&self) -> Arc<RwLock<AgentRegistry>> {
        self.agents.registry.clone()
    }

    /// Deprecated: use `self.agents.manager` instead.
    pub fn session_manager(&self) -> Arc<RwLock<SessionManager>> {
        self.agents.manager.clone()
    }

    /// Deprecated: use `self.agents.group_manager` instead.
    pub fn group_session_manager(&self) -> Arc<RwLock<GroupSessionManager>> {
        self.agents.group_manager.clone()
    }

    /// Deprecated: use `self.agents.store` instead.
    pub fn session_store(&self) -> Option<Arc<SessionStore>> {
        self.agents.store.clone()
    }

    /// Deprecated: use `self.agents.message_buffer` instead.
    pub fn session_message_buffer(&self) -> Arc<RwLock<HashMap<String, Vec<crate::gateway::BufferedMessage>>>> {
        self.agents.message_buffer.clone()
    }

    /// Deprecated: use `self.agents.route_resolver` instead.
    pub fn route_resolver(&self) -> Arc<RouteResolver> {
        self.agents.route_resolver.clone()
    }

    /// Deprecated: use `self.agents.cost_guard` instead.
    pub fn cost_guard(&self) -> Arc<CostGuard> {
        self.agents.cost_guard.clone()
    }

    /// Deprecated: use `self.agents.repair_state` instead.
    pub fn repair_state(&self) -> Arc<RepairState> {
        self.agents.repair_state.clone()
    }

    /// Deprecated: use `self.agents.acp` instead.
    pub fn acp(&self) -> Arc<AcpControlPlane> {
        self.agents.acp.clone()
    }

    /// Deprecated: use `self.agents.session_routing` instead.
    pub fn session_routing(&self) -> Arc<RwLock<HashMap<String, String>>> {
        self.agents.session_routing.clone()
    }

    /// Deprecated: use `self.channels.channels` instead.
    pub fn channels(&self) -> Arc<RwLock<HashMap<String, Arc<dyn Channel>>>> {
        self.channels.channels.clone()
    }

    /// Deprecated: use `self.channels.extensions` instead.
    pub fn channel_extensions(&self) -> Arc<RwLock<ChannelExtensionRegistry>> {
        self.channels.extensions.clone()
    }

    /// Deprecated: use `self.channels.reply_dispatcher` instead.
    pub fn reply_dispatcher(&self) -> Arc<ReplyDispatcher> {
        self.channels.reply_dispatcher.clone()
    }

    /// Deprecated: use `self.channels.snapshot_store` instead.
    pub fn snapshot_store(&self) -> Option<AccountSnapshotStore> {
        self.channels.snapshot_store.clone()
    }

    /// Deprecated: use `self.channels.health_monitor` instead.
    pub fn health_monitor(&self) -> Option<Arc<ChannelHealthMonitor>> {
        self.channels.health_monitor.clone()
    }

    /// Deprecated: use `self.channels.acp_bridge` instead.
    pub fn acp_bridge(&self) -> Option<Arc<ChannelAcpBridge>> {
        self.channels.acp_bridge.clone()
    }

    /// Deprecated: use `self.channels.session_channels` instead.
    pub fn session_channels(&self) -> Arc<RwLock<HashMap<String, (String, String)>>> {
        self.channels.session_channels.clone()
    }

    /// Deprecated: use `self.channels.webhook_sessions` instead.
    pub fn webhook_sessions(&self) -> Arc<RwLock<HashMap<String, String>>> {
        self.channels.webhook_sessions.clone()
    }

    /// Deprecated: use `self.memory.vector` instead.
    pub fn vector_memory(&self) -> LateInit<Arc<VectorMemoryService>> {
        self.memory.vector.clone()
    }

    /// Deprecated: use `self.memory.session_search` instead.
    pub fn session_search(&self) -> LateInit<Arc<SessionSearch>> {
        self.memory.session_search.clone()
    }

    /// Deprecated: use `self.memory.manager` instead.
    pub fn memory_manager(&self) -> Arc<RwLock<Option<Arc<MemoryManager>>>> {
        self.memory.manager.clone()
    }

    /// Deprecated: use `self.memory.dream_scheduler` instead.
    pub fn dream_scheduler(&self) -> LateInit<DreamScheduler> {
        self.memory.dream_scheduler.clone()
    }

    /// Deprecated: use `self.memory.dream_metrics` instead.
    pub fn dream_metrics(&self) -> Arc<DreamMetrics> {
        self.memory.dream_metrics.clone()
    }

    /// Deprecated: use `self.memory.standing_order_manager` instead.
    pub fn standing_order_manager(&self) -> LateInit<crate::standing_orders::StandingOrderManager> {
        self.memory.standing_order_manager.clone()
    }

    /// Deprecated: use `self.tools.registry` instead.
    pub fn tool_registry(&self) -> Arc<ToolRegistry> {
        self.tools.registry.clone()
    }

    /// Deprecated: use `self.tools.mcp_manager` instead.
    pub fn mcp_manager(&self) -> Arc<McpManager> {
        self.tools.mcp_manager.clone()
    }

    /// Deprecated: use `self.tools.approval_queue` instead.
    pub fn approval_queue(&self) -> Arc<ApprovalQueue> {
        self.tools.approval_queue.clone()
    }

    /// Deprecated: use `self.tools.skills_manager` instead.
    pub fn skills_manager(&self) -> Arc<RwLock<SkillManager>> {
        self.tools.skills_manager.clone()
    }

    /// Deprecated: use `self.tools.canvas_manager` instead.
    pub fn canvas_manager(&self) -> Arc<CanvasManager> {
        self.tools.canvas_manager.clone()
    }

    /// Deprecated: use `self.tools.computer_adapter` instead.
    pub fn computer_adapter(&self) -> Arc<RwLock<Option<Arc<dyn crate::computer::ComputerAdapter>>>> {
        self.tools.computer_adapter.clone()
    }

    /// Deprecated: use `self.pipelines.inbound` instead.
    pub fn inbound_pipeline(&self) -> Arc<dyn InboundPipeline> {
        self.pipelines.inbound.clone()
    }

    /// Deprecated: use `self.pipelines.outbound` instead.
    pub fn outbound_pipeline(&self) -> Arc<dyn OutboundPipeline> {
        self.pipelines.outbound.clone()
    }

    /// Deprecated: use `self.pipelines.side_effect_executor` instead.
    pub fn side_effect_executor(&self) -> Arc<SideEffectExecutor> {
        self.pipelines.side_effect_executor.clone()
    }

    /// Deprecated: use `self.pipelines.sse_streamer` instead.
    pub fn sse_streamer(&self) -> Arc<SseStreamer> {
        self.pipelines.sse_streamer.clone()
    }

    /// Deprecated: use `self.pipelines.routed_tx` instead.
    pub fn routed_tx(&self) -> mpsc::Sender<RoutedMessage> {
        self.pipelines.routed_tx.clone()
    }

    /// Deprecated: use `self.pipelines.inbound_entry` instead.
    pub fn inbound_entry(&self) -> mpsc::Sender<IncomingMessage> {
        self.pipelines.inbound_entry.clone()
    }

    /// Deprecated: use `self.events.tx` instead.
    pub fn event_tx(&self) -> broadcast::Sender<GatewayEvent> {
        self.events.tx.clone()
    }

    /// Deprecated: use `self.events.log_tx` instead.
    pub fn log_tx(&self) -> broadcast::Sender<String> {
        self.events.log_tx.clone()
    }

    /// Deprecated: use `self.events.hook_registry` instead.
    pub fn hook_registry(&self) -> Arc<EventHookRegistry> {
        self.events.hook_registry.clone()
    }

    /// Deprecated: use `self.infra.storage` instead.
    pub fn storage(&self) -> Arc<RwLock<dyn Storage>> {
        self.infra.storage.clone()
    }

    /// Deprecated: use `self.infra.runtime_settings` instead.
    pub fn runtime_settings(&self) -> Arc<RwLock<HashMap<String, serde_json::Value>>> {
        self.infra.runtime_settings.clone()
    }

    /// Deprecated: use `self.infra.transcript_store` instead.
    pub fn transcript_store(&self) -> Arc<TranscriptStore> {
        self.infra.transcript_store.clone()
    }

    /// Deprecated: use `self.infra.artifact_store` instead.
    pub fn artifact_store(&self) -> Arc<ArtifactStore> {
        self.infra.artifact_store.clone()
    }

    /// Deprecated: use `self.infra.disk_budget` instead.
    pub fn disk_budget(&self) -> Arc<DiskBudgetManager> {
        self.infra.disk_budget.clone()
    }

    /// Deprecated: use `self.infra.session_file_manager` instead.
    pub fn session_file_manager(&self) -> Arc<SessionFileManager> {
        self.infra.session_file_manager.clone()
    }

    /// Deprecated: use `self.infra.hot_reload` instead.
    pub fn hot_reload(&self) -> LateInit<Arc<HotReloadManager>> {
        self.infra.hot_reload.clone()
    }

    /// Deprecated: use `self.infra.plugin_manager` instead.
    pub fn plugin_manager(&self) -> Arc<PluginManager> {
        self.infra.plugin_manager.clone()
    }

    /// Deprecated: use `self.infra.model_router` instead.
    pub fn model_router(&self) -> Arc<ModelRouter> {
        self.infra.model_router.clone()
    }

    /// Deprecated: use `self.infra.engine_metrics` instead.
    pub fn engine_metrics(&self) -> Option<Arc<crate::core::EngineMetrics>> {
        self.infra.engine_metrics.clone()
    }

    /// Deprecated: use `self.sdk.provider_sdk` instead.
    pub fn provider_sdk(&self) -> Arc<RwLock<ProviderSdk>> {
        self.sdk.provider_sdk.clone()
    }

    /// Deprecated: use `self.sdk.tool_sdk` instead.
    pub fn tool_sdk(&self) -> Arc<RwLock<ToolSdk>> {
        self.sdk.tool_sdk.clone()
    }

    /// Deprecated: use `self.scheduler.task_scheduler` instead.
    pub fn task_scheduler(&self) -> LateInit<Arc<Mutex<TaskScheduler>>> {
        self.scheduler.task_scheduler.clone()
    }

    /// Deprecated: use `self.scheduler.heartbeat_wake_tx` instead.
    pub fn heartbeat_wake_tx(&self) -> LateInit<mpsc::Sender<WakeRequest>> {
        self.scheduler.heartbeat_wake_tx.clone()
    }

    /// Deprecated: use `self.scheduler.heartbeat_event_tx` instead.
    pub fn heartbeat_event_tx(&self) -> LateInit<broadcast::Sender<HeartbeatEvent>> {
        self.scheduler.heartbeat_event_tx.clone()
    }

    /// Deprecated: use `self.scheduler.cron_scheduler` instead.
    pub fn cron_scheduler(&self) -> LateInit<Arc<Mutex<CronScheduler>>> {
        self.scheduler.cron_scheduler.clone()
    }
}
