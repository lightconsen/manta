//! Domain-grouped sub-states for `GatewayState`.
//!
//! This module splits the monolithic flat gateway state into cohesive,
//! lifecycle-aligned sub-states (auth, agents, channels, memory, tools,
//! pipelines, events, infrastructure, sdk, scheduler). Each sub-state owns
//! the services that belong together, making initialization, shutdown, and
//! testing easier to reason about.
//!
//! Handlers access nested state directly, e.g. `state.auth.manager`,
//! `state.tools.registry`, `state.pipelines.inbound`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::acp::AcpControlPlane;
use crate::adapters::Storage;
use crate::agent::{
    session_store::SessionStore, AgentRegistry, ArtifactStore, CostGuard, DiskBudgetManager,
    GroupSessionManager, RouteResolver, SessionFileManager, SessionManager, TranscriptStore,
};
use crate::canvas::CanvasManager;
use crate::channels::snapshot::AccountSnapshotStore;
use crate::channels::{
    Channel, ChannelAcpBridge, ChannelExtensionRegistry, ChannelHealthMonitor, IncomingMessage,
};
use crate::config::hot_reload::HotReloadManager;
use crate::cron::cron::CronScheduler;
use crate::device::control::ControlHandlerRegistry;
use crate::device::DriverFactory;
use crate::gateway::hooks::EventHookRegistry;
use crate::gateway::init::devices::DeviceInit;
use crate::gateway::rate_limit::MultiTierRateLimiter;
use crate::gateway::task_registry::TaskRegistry;
use crate::gateway::AgentHandle;
use crate::gateway::RepairState;
use crate::gateway::{GatewayConfig, GatewayEvent};
use crate::heartbeat::{HeartbeatEvent, WakeRequest};
use crate::inbound::{AgentRouter, InboundPipeline, RoutedMessage};
use crate::memory::vector::VectorMemoryService;
use crate::memory::{DreamMetrics, MemoryManager, SessionSearch};
use crate::model_router::ModelRouter;
use crate::outbound::{OutboundPipeline, ReplyDispatcher, SideEffectExecutor, SseStreamer};
use crate::planner::TaskScheduler;
use crate::plugins::PluginManager;
use crate::providers::ProviderSdk;
use crate::security::device_pairing::DevicePairingStore;
use crate::security::{
    mention_gate::MentionGate, pairing::PairingStore, persistent_audit::PersistentAuditLog,
    tailscale::TailscaleAuthenticator, trusted_proxy::TrustedProxyAuthenticator, AuthManager,
    RateLimiter,
};
use crate::skills::SkillManager;
use crate::tools::{
    approval::ApprovalQueue, command_gate::CommandGate, mcp::McpManager, ToolRegistry, ToolSdk,
};

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
    /// Per-session delayed flush timers for FollowUp queue mode.
    pub follow_up_timers: Arc<RwLock<HashMap<String, JoinHandle<()>>>>,
    pub route_resolver: Arc<RouteResolver>,
    pub cost_guard: Arc<CostGuard>,
    pub repair_state: Arc<RepairState>,
    pub acp: Arc<AcpControlPlane>,
}

/// Channel adapters, extensions, and response dispatch state.
pub struct ChannelState {
    pub channels: Arc<RwLock<HashMap<String, Arc<dyn Channel>>>>,
    pub extensions: Arc<RwLock<ChannelExtensionRegistry>>,
    pub reply_dispatcher: Arc<ReplyDispatcher>,
    pub snapshot_store: Option<AccountSnapshotStore>,
    pub health_monitor: Option<Arc<ChannelHealthMonitor>>,
    pub acp_bridge: Option<Arc<ChannelAcpBridge>>,
    /// Session to channel mapping: session_id -> (channel_name,
    /// channel_specific_id).
    pub session_channels: Arc<RwLock<HashMap<String, (String, String)>>>,
    /// Webhook session storage: platform_key -> session_uuid.
    pub webhook_sessions: Arc<RwLock<HashMap<String, String>>>,
}

/// Memory, search, and background consolidation state.
pub struct MemoryState {
    pub vector: RwLock<Option<Arc<VectorMemoryService>>>,
    pub session_search: RwLock<Option<Arc<SessionSearch>>>,
    pub manager: Arc<RwLock<Option<Arc<MemoryManager>>>>,
    pub dream_scheduler: RwLock<Option<crate::memory::DreamScheduler>>,
    pub dream_metrics: Arc<DreamMetrics>,
    pub standing_order_manager: RwLock<Option<crate::standing_orders::StandingOrderManager>>,
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
    pub hot_reload: RwLock<Option<Arc<HotReloadManager>>>,
    pub plugin_manager: Arc<PluginManager>,
    pub model_router: Arc<ModelRouter>,
    /// Pluggable device driver factory — shared between config-driven init
    /// and OS bridge runtime event path.
    pub driver_factory: DriverFactory,
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
    pub task_scheduler: RwLock<Option<Arc<Mutex<TaskScheduler>>>>,
    pub heartbeat_wake_tx: RwLock<Option<mpsc::Sender<WakeRequest>>>,
    pub heartbeat_event_tx: RwLock<Option<broadcast::Sender<HeartbeatEvent>>>,
    pub cron_scheduler: RwLock<Option<Arc<Mutex<CronScheduler>>>>,
}

/// Perception subsystem init state (registry, streaming context,
/// background poll handle).
/// Replaced on hot-reload via the admin API.
pub struct PerceptionInit {
    /// The perception registry managing sources and scene graph.
    pub registry: Arc<crate::perception::PerceptionRegistry>,
    /// Shared streaming infrastructure (raw_hub + derived_hub +
    /// temporal processor + fusion engine). Per-agent
    /// [`crate::perception::AgentPerceptionAdapter`]s are minted from
    /// this context.
    pub context: Arc<crate::perception::PerceptionContext>,
    /// Background poll loop handle, if one was spawned.
    pub poll_handle: Option<JoinHandle<()>>,
}

/// Control lane init state (registry, runtime, loop handle).
/// Replaced on hot-reload via the admin API.
pub struct ControlInit {
    /// The device registry shared with the control lane.
    pub registry: Arc<crate::device::registry::DeviceRegistry>,
    /// The control loop join handle.
    pub handle: Option<tokio::task::JoinHandle<()>>,
    /// Registered control handlers (shared with driver connections).
    pub handlers: ControlHandlerRegistry,
}

/// Shared gateway state grouped by domain.
pub struct GatewayState {
    /// Configuration
    pub config: Arc<RwLock<GatewayConfig>>,
    /// Gateway startup time for uptime calculations
    pub start_time: Instant,
    /// Path to the config file (for runtime persistence)
    pub config_path: Option<PathBuf>,
    /// Device subsystem init state (registry, health check handle).
    /// Replaced on hot-reload to re-probe/re-connect devices.
    pub device_init: RwLock<Option<DeviceInit>>,
    /// Perception fusion layer init state (registry, poll loop handle).
    /// Replaced on hot-reload via the admin API.
    pub perception_init: RwLock<Option<PerceptionInit>>,
    /// Control lane init state (runtime, handlers, loop handle).
    /// Replaced on hot-reload via the admin API.
    pub control_init: RwLock<Option<ControlInit>>,

    /// Centralized registry for all gateway background tasks.
    pub task_registry: Arc<TaskRegistry>,

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
