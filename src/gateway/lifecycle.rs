//! Gateway lifecycle functions — start, stop, build_router.
//!
//! Extracted from `gateway/mod.rs` to reduce the main module size. Each
//! function takes the pieces of [`Gateway`](super::Gateway) it needs
//! explicitly (state, config, shutdown_token, task-trackers) instead of
//! `&self`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    middleware::{from_fn, from_fn_with_state},
    routing::{delete, get, post},
    Router,
};
use tokio::net::TcpListener;
use tokio::time::{timeout, Duration};
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

use super::agent_spawn::spawn_agent_inner;
use super::*;
use super::{GatewayConfig, GatewayState};
use crate::agent::AgentConfig;
use crate::config::hot_reload::ConfigFileType;
use crate::tools::mcp::McpToolWrapper;

// ── start ────────────────────────────────────────────────────────────

/// Start the gateway and all its subsystems.
pub(crate) async fn start_gateway(
    state: Arc<GatewayState>,
    config: GatewayConfig,
    shutdown_token: CancellationToken,
) -> crate::Result<()> {
    info!("Starting Syscity Gateway control plane...");

    // Initialize plugins if enabled
    if config.plugins.enabled {
        if config.plugins.auto_load {
            if let Err(e) = state.infra.plugin_manager.initialize().await {
                warn!("Failed to initialize plugins: {}", e);
            }

            // Watch WASM files for hot-reload
            if let Some(hot_reload) = state.infra.hot_reload.read().await.clone() {
                let plugins = state.infra.plugin_manager.list_plugins().await;
                for plugin in plugins {
                    if let Some(ref main) = plugin.manifest.main {
                        let wasm_path = plugin.path.join(main);
                        if wasm_path.exists() {
                            if let Err(e) = hot_reload
                                .watch_file(&wasm_path, ConfigFileType::Plugin)
                                .await
                            {
                                warn!(
                                    "Failed to watch WASM file for plugin '{}': {}",
                                    plugin.id(),
                                    e
                                );
                            } else {
                                debug!(
                                    "Watching WASM file for plugin '{}': {:?}",
                                    plugin.id(),
                                    wasm_path
                                );
                            }
                        }
                    }
                }
            }
        } else {
            info!("Plugin auto-load disabled, skipping initialization");
        }
    } else {
        info!("Plugin system disabled");
    }

    // Initialize skills manager
    {
        let mut skills_manager = state.tools.skills_manager.write().await;
        match skills_manager.initialize().await {
            Ok(count) => info!("✅ Skills manager initialized with {} skills", count),
            Err(e) => warn!("Failed to initialize skills manager: {}", e),
        }
    }

    // Start model-router health checks. They are registered with the task
    // registry and respect the gateway shutdown token.
    state.infra.model_router.clone().start_health_checks();

    // Initialize hot reload if enabled
    let hot_reload = state.infra.hot_reload.read().await.clone();
    if let Some(ref hot_reload) = hot_reload {
        let config_path = crate::dirs::default_config_file();
        if let Err(e) = hot_reload
            .watch_file(&config_path, ConfigFileType::Main)
            .await
        {
            warn!("Failed to watch config file: {}", e);
        }
        // Start hot reload processing in background
        let hot_reload_clone = hot_reload.clone();
        let hot_reload_handle = tokio::spawn(async move {
            if let Err(e) = hot_reload_clone.run().await {
                error!("Hot reload error: {}", e);
            }
        });
        state
            .task_registry
            .insert_join("hot_reload", hot_reload_handle)
            .await;

        // Register config change handlers
        super::hot_reload::register_hot_reload_handlers(state.clone(), config.clone(), hot_reload)
            .await;
    }

    // Initialize default agent (optional - requires provider configuration)
    let mut default_config = config.default_agent.clone();
    let default_agent_dir = crate::dirs::agents_dir().join("default");
    default_config.system_prompt = format!(
        "{}\n\n## Agent Identity\n\nYour agent ID is: `default`\nYour agent directory is: \
         `{}`\nYou may edit files in your agent directory (including HEARTBEAT.md) to manage your \
         personality and periodic tasks when explicitly asked by the user.",
        default_config.system_prompt,
        default_agent_dir.display()
    );
    match spawn_agent_in_lifecycle(state.clone(), "default".to_string(), default_config).await {
        Ok(()) => info!("Default agent spawned successfully"),
        Err(e) => {
            warn!("Failed to spawn default agent: {}", e);
            warn!("Gateway running without default agent - agents must be created via API");
        }
    }

    // Discover agents from agents/ directory (auto-discovery)
    {
        let mut registry = state.agents.registry.write().await;
        match registry.discover().await {
            Ok(count) => {
                if count > 0 {
                    info!("🔍 Discovered {} agents from agents/ directory", count);
                    // List discovered agents
                    for id in registry.list() {
                        if let Some(personality) = registry.get(&id) {
                            info!("  📋 Agent '{}' - {}", id, personality.display_name());
                        }
                    }
                } else {
                    info!("🔍 No agents found in agents/ directory");
                }
            }
            Err(e) => {
                warn!("Failed to discover agents: {}", e);
            }
        }
    }

    // Register delegation tool with agent resolver for target_agent routing.
    {
        use crate::tools::DelegateTool;
        let resolver = Arc::new(super::agent_spawn::GatewayAgentResolver {
            agents: state.agents.agents.clone(),
        });
        let default_agent = {
            let agents = state.agents.agents.read().await;
            agents.get("default").map(|h| h.agent.clone())
        };
        let delegate = if let Some(agent) = default_agent {
            DelegateTool::with_agent(0, agent).with_agent_resolver(resolver)
        } else {
            DelegateTool::root().with_agent_resolver(resolver)
        };
        state.tools.registry.register_dynamic(Arc::new(delegate));
        info!("DelegateTool registered with agent resolver for target_agent routing");
    }

    // Auto-connect MCP servers
    init_mcp_servers(&state, &config).await;

    // Initialize configured channels
    super::init::channels::init_channels(state.clone(), &config).await?;

    // Start dream scheduler if enabled
    if config.dreaming.enabled {
        if let Some(mm) = state.memory.manager.read().await.as_ref().cloned() {
            if let Some(tier_index) = mm.tier_index() {
                let dreaming = &config.dreaming;
                let speed = match dreaming.speed.to_lowercase().as_str() {
                    "fast" => crate::memory::DreamSpeed::Fast,
                    "slow" => crate::memory::DreamSpeed::Slow,
                    _ => crate::memory::DreamSpeed::Balanced,
                };
                let thinking = match dreaming.thinking.to_lowercase().as_str() {
                    "low" => crate::memory::DreamThinking::Low,
                    "high" => crate::memory::DreamThinking::High,
                    _ => crate::memory::DreamThinking::Medium,
                };
                let budget = match dreaming.budget.to_lowercase().as_str() {
                    "cheap" => crate::memory::DreamBudget::Cheap,
                    "expensive" => crate::memory::DreamBudget::Expensive,
                    _ => crate::memory::DreamBudget::Medium,
                };
                let dream_config = crate::memory::DreamConfig {
                    enabled: dreaming.enabled,
                    frequency: dreaming.frequency.clone(),
                    speed,
                    thinking,
                    budget,
                    dedup_similarity_threshold: dreaming.dedup_similarity_threshold,
                    ..crate::memory::DreamConfig::default()
                };
                let tier_system_config = crate::memory::TierSystemConfig::default();
                let mut engine = crate::memory::DreamEngine::new(dream_config, tier_system_config)
                    .with_metrics(Arc::clone(&state.memory.dream_metrics));
                if let Some(ref workspace_dir) = config.workspace_dir {
                    engine = engine.with_workspace_dir(workspace_dir.clone());
                }
                if let Some(event_log) = mm.event_log() {
                    engine = engine.with_event_log(event_log.clone());
                }
                engine.initialize().await;
                let engine = Arc::new(engine);
                let mut scheduler = crate::memory::DreamScheduler::new(engine);
                let handle = scheduler.start(mm.store(), tier_index);
                state
                    .task_registry
                    .insert_join("dream_scheduler", handle)
                    .await;
                info!("Dream scheduler started");
                *state.memory.dream_scheduler.write().await = Some(scheduler);
            }
        }
    }

    // Start standing orders manager if configured
    if config.standing_orders.enabled {
        let mut manager = crate::standing_orders::StandingOrderManager::new(
            config.standing_orders.clone(),
            state.clone(),
        );
        manager.start();
        info!("Standing orders manager started");
        *state.memory.standing_order_manager.write().await = Some(manager);
    }

    // Start browser bridge server if enabled
    #[cfg(feature = "browser")]
    if config.browser.bridge_enabled {
        let pool = Arc::new(crate::browser::BrowserPool::with_profiles(
            config.browser.pool.clone(),
            config.browser.profiles.clone(),
        ));
        let mut bridge = crate::browser::BrowserBridge::new(pool, config.browser.bridge_port);
        let token = bridge.token().to_string();
        match bridge.start().await {
            Ok(port) => {
                let url = format!("http://127.0.0.1:{}", port);
                info!(port = port, "Browser bridge server started");
                {
                    let mut bridge_lock = state.infra.browser_bridge.write().await;
                    *bridge_lock = Some(bridge);
                }
                let mut settings = state.infra.runtime_settings.write().await;
                settings.insert("browser_bridge_url".to_string(), serde_json::json!(url));
                settings.insert("browser_bridge_token".to_string(), serde_json::json!(token));
            }
            Err(e) => {
                warn!("Failed to start browser bridge server: {}", e);
            }
        }
    }

    // Build HTTP router
    let app = build_router(state.clone()).await;

    // Bind to address
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| crate::error::ConfigError::InvalidValue {
            key: "gateway.address".to_string(),
            message: format!("Invalid gateway address: {}", e),
        })?;

    let listener = TcpListener::bind(&addr).await.map_err(|e| {
        crate::error::SyscityError::ExternalService {
            source: "Failed to bind gateway".to_string(),
            cause: Some(Box::new(e)),
        }
    })?;

    info!("Gateway control plane listening on ws://{}", addr);

    // Forward ApprovalRequired events from the tool registry into the Gateway event
    // bus
    {
        let mut approval_rx = state.tools.approval_queue.event_tx.subscribe();
        let event_tx = state.events.tx.clone();
        let shutdown_token = shutdown_token.clone();
        let approval_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        info!("Approval forwarder received shutdown signal, exiting");
                        break;
                    }
                    result = approval_rx.recv() => {
                        let evt = match result {
                            Ok(evt) => evt,
                            Err(_) => break,
                        };
                        if let Err(e) = event_tx.send(crate::gateway::GatewayEvent::ApprovalRequired {
                            approval_id: evt.approval_id,
                            tool_name: evt.tool_name,
                            requested_by: evt.requested_by,
                            risk_level: evt.risk_level,
                            message: evt.message,
                        }) {
                            debug!("No receivers for ApprovalRequired event: {}", e);
                        }
                    }
                }
            }
        });
        state
            .task_registry
            .insert_join("approval_forwarder", approval_handle)
            .await;
    }

    // Start gateway-level self-repair watchdog (60 s interval)
    let repair_handle =
        tokio::spawn(super::watchdog::run_repair_loop(state.clone(), shutdown_token.clone()));
    state
        .task_registry
        .insert_join("repair_loop", repair_handle)
        .await;

    // Start heartbeat runner if enabled
    if config.heartbeat.enabled {
        let runner = crate::heartbeat::HeartbeatRunner::new(state.clone());
        let wake_tx = runner.wake_sender();
        let event_tx = runner.event_tx.clone();
        *state.scheduler.heartbeat_wake_tx.write().await = Some(wake_tx.clone());
        *state.scheduler.heartbeat_event_tx.write().await = Some(event_tx);
        let heartbeat_handle = tokio::spawn(async move {
            runner.start().await;
        });
        state
            .task_registry
            .insert_join("heartbeat", heartbeat_handle)
            .await;
        info!("Heartbeat runner started");

        // Wire heartbeat wake sender into cron scheduler
        if let Some(cron_arc) = state.scheduler.cron_scheduler.read().await.clone() {
            let mut scheduler = cron_arc.lock().await;
            scheduler.set_heartbeat_wake_tx(wake_tx);
            info!("Cron heartbeat wake integration enabled");
        }
    }

    // Start log tail broadcaster for real-time log streaming
    {
        let log_tx = state.events.log_tx.clone();
        let shutdown_token = shutdown_token.clone();
        let log_tail_handle = tokio::spawn(async move {
            let log_path = crate::logs::log_file_path();
            let mut pos: u64 = 0;
            loop {
                tokio::select! {
                    _ = shutdown_token.cancelled() => {
                        info!("Log tail broadcaster received shutdown signal, exiting");
                        break;
                    }
                    _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {
                        if log_path.exists() {
                            match tokio::fs::metadata(&log_path).await {
                                Ok(meta) => {
                                    let new_len = meta.len();
                                    if new_len > pos {
                                        match tokio::fs::File::open(&log_path).await {
                                            Ok(file) => {
                                                let mut reader = tokio::io::BufReader::new(file);
                                                if let Err(e) =
                                                    reader.seek(tokio::io::SeekFrom::Start(pos)).await
                                                {
                                                    tracing::warn!("Log tail seek error: {}", e);
                                                } else {
                                                    let mut lines = reader.lines();
                                                    while let Ok(Some(line)) = lines.next_line().await {
                                                        if let Err(e) = log_tx.send(line) {
                                                            debug!(
                                                                "No receivers for log tail event: {}",
                                                                e
                                                            );
                                                        }
                                                    }
                                                }
                                                pos = new_len;
                                            }
                                            Err(e) => {
                                                tracing::warn!("Log tail open error: {}", e);
                                            }
                                        }
                                    } else if new_len < pos {
                                        // File was truncated/rotated
                                        pos = 0;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Log tail metadata error: {}", e);
                                }
                            }
                        }
                    }
                }
            }
        });
        state
            .task_registry
            .insert_join("log_tail", log_tail_handle)
            .await;
        info!("Log tail broadcaster started");
    }

    // Run the server with graceful shutdown (bounded by a 30s timeout so a
    // stuck connection cannot prevent gateway teardown indefinitely).
    let shutdown = async move { shutdown_token.cancelled().await };
    match timeout(
        Duration::from_secs(30),
        axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
            .with_graceful_shutdown(shutdown),
    )
    .await
    {
        Ok(result) => result.map_err(|e| crate::error::SyscityError::ExternalService {
            source: "Gateway server error".to_string(),
            cause: Some(Box::new(e)),
        })?,
        Err(_) => {
            warn!("Axum graceful shutdown timed out after 30s; proceeding with teardown");
        }
    }

    // Stop dream scheduler on shutdown
    if let Some(mut scheduler) = state.memory.dream_scheduler.read().await.clone() {
        scheduler.stop().await;
        if let Some(handle) = state
            .task_registry
            .remove_join_or_abort("dream_scheduler")
            .await
        {
            match timeout(Duration::from_secs(5), handle).await {
                Ok(_) => info!("Dream scheduler stopped"),
                Err(_) => warn!("Dream scheduler did not stop within timeout"),
            }
        } else {
            info!("Dream scheduler stopped");
        }
    }

    // Stop standing orders manager on shutdown
    if let Some(mut manager) = state.memory.standing_order_manager.write().await.take() {
        manager.stop().await;
        info!("Standing orders manager stopped");
    }

    Ok(())
}

// ── stop ─────────────────────────────────────────────────────────────

/// Gracefully shut down the gateway and its subsystems.
pub(crate) async fn stop_gateway(
    shutdown_token: &CancellationToken,
    state: &Arc<GatewayState>,
) -> crate::Result<()> {
    info!("Shutting down Syscity Gateway...");

    // Signal every cancel-aware loop to exit.
    shutdown_token.cancel();

    // 1. Drain the unified message workers.
    let message_handles = state
        .task_registry
        .remove_matching_join_or_abort("message:")
        .await;
    for handle in message_handles {
        match timeout(Duration::from_secs(5), handle).await {
            Ok(_) => {}
            Err(_) => warn!("Message worker did not stop within timeout"),
        }
    }

    // 2. Stop all spawned agents and await their loops.
    {
        let agents = state.agents.agents.read().await;
        for (id, handle) in agents.iter() {
            if let Err(e) = handle.tx.send(crate::gateway::AgentCommand::Shutdown).await {
                warn!("Failed to send shutdown to agent {}: {}", id, e);
            }
        }
    }
    let agent_handles = state
        .task_registry
        .remove_matching_join_or_abort("agent:")
        .await;
    for handle in agent_handles {
        match timeout(Duration::from_secs(10), handle).await {
            Ok(_) => {}
            Err(_) => warn!("Agent task did not stop within timeout"),
        }
    }

    // 3. Stop configured channels.
    // Abort channel background tasks first so the channel stop() calls do not
    // race with gateway-owned inbound/outbound bridges.
    let channel_handles = state
        .task_registry
        .remove_matching_join_or_abort("channel:")
        .await;
    for handle in channel_handles {
        handle.abort();
    }
    let channel_refs: Vec<Arc<dyn crate::channels::Channel>> = {
        let channels = state.channels.channels.read().await;
        channels.values().cloned().collect()
    };
    for channel in channel_refs {
        let name = channel.name().to_string();
        if let Err(e) = channel.stop().await {
            warn!("Failed to stop channel '{}': {}", name, e);
        } else {
            info!("Channel '{}' stopped", name);
        }
    }

    // 4. ACP shutdown.
    if let Err(e) = state.agents.acp.shutdown().await {
        warn!("Failed to shut down ACP control plane: {}", e);
    } else {
        info!("ACP control plane shut down");
    }

    // 5. Cron scheduler.
    if let Some(cron_arc) = state.scheduler.cron_scheduler.read().await.clone() {
        let mut scheduler = cron_arc.lock().await;
        if let Err(e) = scheduler.shutdown().await {
            warn!("Failed to shutdown cron scheduler: {}", e);
        } else {
            info!("Cron scheduler stopped");
        }
    }

    // 6. Dream scheduler.
    if let Some(mut scheduler) = state.memory.dream_scheduler.read().await.clone() {
        scheduler.stop().await;
        if let Some(handle) = state
            .task_registry
            .remove_join_or_abort("dream_scheduler")
            .await
        {
            match timeout(Duration::from_secs(5), handle).await {
                Ok(_) => info!("Dream scheduler stopped"),
                Err(_) => warn!("Dream scheduler did not stop within timeout"),
            }
        } else {
            info!("Dream scheduler stopped");
        }
    }

    // 7. Standing orders manager.
    if let Some(mut manager) = state.memory.standing_order_manager.write().await.take() {
        manager.stop().await;
        info!("Standing orders manager stopped");
    }

    // 8. Disconnect MCP servers.
    let mcp_servers = state.tools.mcp_manager.list_servers().await;
    for server_id in mcp_servers {
        if let Err(e) = state.tools.mcp_manager.disconnect(&server_id).await {
            warn!("Failed to disconnect MCP server '{}': {}", server_id, e);
        }
    }

    // 9. Hot reload.
    if let Some(hot_reload) = state.infra.hot_reload.read().await.clone() {
        if let Err(e) = hot_reload.stop().await {
            warn!("Failed to stop hot reload manager: {}", e);
        }
    }

    // 10. Task scheduler.
    if let Some(ts_arc) = state.scheduler.task_scheduler.read().await.clone() {
        let mut scheduler = ts_arc.lock().await;
        if let Err(e) = scheduler.stop().await {
            warn!("Failed to stop task scheduler: {}", e);
        }
    }

    // 11. Browser bridge / pool.
    #[cfg(feature = "browser")]
    {
        let mut bridge_lock = state.infra.browser_bridge.write().await;
        if let Some(bridge) = bridge_lock.take() {
            bridge.shutdown().await;
            info!("Browser pool shut down");
        }
    }

    // 12. Abort remaining background tasks (includes followup timers now that they
    //     live in the unified registry).
    let background_handles = state.task_registry.take_all().await;
    for (_name, handle) in background_handles {
        handle.abort();
    }

    // 13. Plugin manager shutdown.
    if let Err(e) = state.infra.plugin_manager.shutdown().await {
        warn!("Failed to shutdown plugin manager: {}", e);
    }

    // 14. Storage is left to flush on process exit.
    info!("Gateway shutdown complete");
    Ok(())
}

// ── build_router ─────────────────────────────────────────────────────

/// Build the HTTP router.
pub(crate) async fn build_router(state: Arc<GatewayState>) -> Router {
    // Public tier: Webhooks (no authentication, signature verification per-channel)
    let public_router = super::webhooks::create_webhook_router(state.clone());

    // Auth tier: OAuth login/logout (public-facing, no tailscale restriction)
    let auth_router = Router::new()
        .route("/auth/github", get(super::auth::oauth::github_login_handler))
        .route("/auth/github/callback", get(super::auth::oauth::github_callback_handler))
        .route("/auth/google", get(super::auth::oauth::google_login_handler))
        .route("/auth/google/callback", get(super::auth::oauth::google_callback_handler))
        .route("/auth/logout", post(super::auth::oauth::logout_handler))
        .layer(from_fn_with_state(state.clone(), super::middleware::rate_limit_middleware))
        .layer(from_fn(super::middleware::security_headers_middleware))
        .with_state(state.clone());

    // Admin tier: Essential APIs (not deprecated)
    let essential_public_router = Router::new()
        .route("/health", get(super::health_handler))
        .route("/ready", get(super::ready_handler))
        .route("/live", get(super::live_handler))
        .route("/api/v1/health", get(super::health_handler))
        .route("/api/v1/metrics", get(super::metrics_handler));

    // Authenticated essential APIs (auth required)
    let essential_auth_router = Router::new()
        .route("/v1/chat/completions", post(super::openai_chat_completions_handler))
        .route("/v1/models", get(super::openai_list_models_handler))
        .route("/api/v1/models", get(super::list_models_handler))
        .route("/api/v1/reload", post(super::reload_all_handler))
        .route("/api/v1/channels", get(super::channel_list_handler))
        .route("/api/v1/channels/{name}/enable", post(super::enable_channel_handler))
        .route("/api/v1/channels/{name}/disable", post(super::disable_channel_handler))
        .route("/api/v1/plugins", get(super::list_plugins_handler))
        .route("/api/v1/plugins/install", post(super::install_plugin_handler))
        .route("/api/v1/plugins/uninstall", post(super::uninstall_plugin_handler))
        .route("/api/v1/plugins/search", get(super::search_plugins_handler))
        .route("/api/v1/plugins/sign", post(super::sign_plugin_handler))
        .route("/api/v1/plugins/reload", post(super::reload_plugins_handler))
        .route("/api/v1/plugins/{name}/enable", post(super::enable_plugin_handler))
        .route("/api/v1/plugins/{name}/disable", post(super::disable_plugin_handler))
        .route("/api/v1/plugins/{name}/unload", delete(super::unload_plugin_handler))
        .route("/api/v1/plugins/{name}/reload", post(super::reload_plugin_handler))
        .route("/api/v1/skills", get(super::list_skills_handler))
        .route("/api/v1/skills/install", post(super::install_skill_handler))
        .route("/api/v1/skills/{name}", get(super::get_skill_handler))
        .route("/api/v1/skills/{name}/enable", post(super::enable_skill_handler))
        .route("/api/v1/skills/{name}/disable", post(super::disable_skill_handler))
        .route("/api/v1/skills/{name}/run", post(super::run_skill_handler))
        .route("/api/v1/skills/{name}/uninstall", post(super::uninstall_skill_handler))
        .route("/api/v1/device/pairing/pending", get(super::list_device_pending_handler))
        .route("/api/v1/device/pairing/authorized", get(super::list_device_authorized_handler))
        .route("/api/v1/device/pairing/approve", post(super::approve_device_handler))
        .route("/api/v1/device/pairing/reject", post(super::reject_device_handler))
        .route("/api/v1/device/pairing/revoke", post(super::revoke_device_handler))
        .route("/api/v1/device/pairing/qr/{code}", get(super::device_qr_handler))
        .route("/api/v1/device/pairing/setup/{setup_code}", get(super::setup_device_handler))
        .layer(from_fn_with_state(state.clone(), super::middleware::auth_middleware));

    let essential_router = essential_public_router.merge(essential_auth_router);

    // Apply remaining middleware layers to essential routes
    let admin_router = essential_router
        .layer(from_fn_with_state(state.clone(), super::middleware::rate_limit_middleware))
        .layer(from_fn_with_state(state.clone(), super::auth::session_cookie_middleware))
        .layer(from_fn_with_state(state.clone(), super::middleware::tailscale_auth_middleware))
        .layer(from_fn_with_state(
            state.clone(),
            super::middleware::trusted_proxy_auth_middleware,
        ))
        .layer(from_fn(super::middleware::security_headers_middleware))
        .with_state(state.clone());

    // WebSocket sub-router with mandatory auth validation middleware
    let ws_router = Router::new()
        .route("/ws", get(super::ws::ws_handler))
        .layer(from_fn_with_state(state.clone(), super::ws::ws_auth_middleware))
        .with_state(state.clone());

    // Build CORS layer from config
    let cors_layer = {
        let config = state.config.read().await;
        if config.security.cors.enabled {
            let mut cors = CorsLayer::new();
            if config.security.cors.allow_credentials {
                cors = cors.allow_credentials(true);
            }
            let has_wildcard = config
                .security
                .cors
                .allowed_origins
                .iter()
                .any(|o| o == "*");
            if has_wildcard && config.security.cors.allow_credentials {
                cors = cors.allow_origin(tower_http::cors::AllowOrigin::mirror_request());
            } else {
                for origin in &config.security.cors.allowed_origins {
                    if origin == "*" {
                        cors = cors.allow_origin(tower_http::cors::Any);
                    } else if let Ok(header_value) = origin.parse() {
                        cors = cors.allow_origin([header_value]);
                    }
                }
            }
            let methods: Vec<_> = config
                .security
                .cors
                .allowed_methods
                .iter()
                .filter_map(|m| m.parse().ok())
                .collect();
            if !methods.is_empty() {
                cors = cors.allow_methods(methods);
            }
            let headers: Vec<_> = config
                .security
                .cors
                .allowed_headers
                .iter()
                .filter_map(|h| h.parse().ok())
                .collect();
            if !headers.is_empty() {
                cors = cors.allow_headers(headers);
            }
            cors.max_age(std::time::Duration::from_secs(config.security.cors.max_age_secs as u64))
        } else {
            CorsLayer::new()
        }
    };

    // SPA frontend routes (serve built React app from embedded assets)
    let frontend_router = Router::new()
        .route("/", get(super::web_terminal_html_handler))
        .route("/favicon.svg", get(super::favicon_handler))
        .route("/syscity.png", get(super::syscity_png_handler))
        .route("/manifest.webmanifest", get(super::manifest_handler))
        .route("/registerSW.js", get(super::register_sw_handler))
        .route("/assets/*path", get(super::asset_handler));

    // Merge all routers and apply global CORS
    frontend_router
        .merge(public_router)
        .merge(auth_router)
        .merge(admin_router)
        .merge(ws_router)
        .layer(cors_layer)
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Spawn an agent and track its task handle.
async fn spawn_agent_in_lifecycle(
    state: Arc<GatewayState>,
    id: String,
    config: AgentConfig,
) -> crate::Result<()> {
    spawn_agent_inner(state, id, config).await?;
    Ok(())
}

/// Auto-connect MCP servers from config and register their tools.
pub(crate) async fn init_mcp_servers(state: &Arc<GatewayState>, config: &GatewayConfig) {
    let servers = &config.mcp.servers;
    if servers.is_empty() {
        debug!("No MCP servers configured");
        return;
    }

    info!("Auto-connecting {} configured MCP server(s)…", servers.len());

    for (server_id, server_config) in servers {
        if !server_config.auto_connect {
            info!("MCP server '{}' has auto_connect=false, skipping", server_id);
            continue;
        }

        match state
            .tools
            .mcp_manager
            .connect(server_id, server_config.clone())
            .await
        {
            Ok(tools) => {
                info!(
                    "✅ MCP server '{}' connected: {} tool(s) discovered",
                    server_id,
                    tools.len()
                );

                let max_tools = if server_config.max_tools == 0 {
                    tools.len()
                } else {
                    server_config.max_tools.min(tools.len())
                };

                if let Some(client_arc) = state.tools.mcp_manager.get_client(server_id).await {
                    for tool in tools.iter().take(max_tools) {
                        let wrapper =
                            Arc::new(McpToolWrapper::new(client_arc.clone(), server_id, tool));
                        state.tools.registry.register_dynamic(wrapper);
                        debug!("  Registered MCP tool: mcp__{}__{}", server_id, tool.name);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to connect MCP server '{}': {}", server_id, e);
            }
        }
    }
}
