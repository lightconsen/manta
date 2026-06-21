//! Channel initialization functions.
//!
//! Extracted from `gateway/mod.rs`. Provides free functions for initializing
//! individual channels by type, as well as the top-level `init_channels` loop.
//! All functions take `state` / `config` explicitly instead of `&self`.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::channels::snapshot::healthy_snapshot;
use crate::channels::{Channel, ChannelExtension, ChannelType};
use crate::gateway::config::ChannelConfig;
use crate::gateway::GatewayState;

/// Initialize all configured channels.
pub(crate) async fn init_channels(
    state: Arc<GatewayState>,
    config: &crate::gateway::config::GatewayConfig,
) -> crate::Result<()> {
    info!("Initializing {} configured channels", config.channels.len());

    for (name, channel_config) in &config.channels {
        if !channel_config.enabled {
            info!("Channel {} is disabled, skipping", name);
            continue;
        }

        // Check if channel already running
        if state.channels.channels.read().await.contains_key(name) {
            info!("Channel {} already running, skipping", name);
            continue;
        }

        init_single_channel(state.clone(), config, name, channel_config).await?;
    }

    // Discover and start WASM plugin channels
    #[cfg(feature = "plugins")]
    init_plugin_channels(state, config).await?;

    Ok(())
}

/// Discover and start WASM plugin channels.
#[cfg(feature = "plugins")]
pub(crate) async fn init_plugin_channels(
    state: Arc<GatewayState>,
    _config: &crate::gateway::config::GatewayConfig,
) -> crate::Result<()> {
    use crate::channels::plugin_host::PluginChannelRegistry;
    use crate::dirs;

    let plugin_dir = dirs::extensions_dir().join("channels");
    if !plugin_dir.exists() {
        info!("Plugin channel directory does not exist, skipping: {:?}", plugin_dir);
        return Ok(());
    }

    // Create a shared inbound message channel for plugin channels.
    // The receiver needs to be wired into the inbound pipeline (see TODO).
    let (plugin_inbound_tx, _plugin_inbound_rx) = tokio::sync::mpsc::unbounded_channel();

    let registry = PluginChannelRegistry::new(plugin_dir, plugin_inbound_tx);
    let available = registry.discover_plugins().await?;

    if available.is_empty() {
        info!("No WASM channel plugins found");
        return Ok(());
    }

    for (name, path) in &available {
        info!("Discovered WASM channel plugin '{}' at {:?}", name, path);
    }

    for (name, _path) in &available {
        // Skip if a native channel with the same name is already running
        if state.channels.channels.read().await.contains_key(name) {
            info!("Channel '{}' already running as native, skipping plugin", name);
            continue;
        }

        match registry.load_plugin(name, None).await {
            Ok(plugin) => {
                info!("Loaded WASM channel plugin '{}'", name);
                // Register in the channel map
                let channel: Arc<dyn crate::channels::Channel> = plugin.clone();
                state
                    .channels
                    .channels
                    .write()
                    .await
                    .insert(name.clone(), channel.clone());

                // Start the plugin channel
                if let Err(e) = plugin.start().await {
                    warn!("Failed to start WASM channel plugin '{}': {}", name, e);
                    continue;
                }

                // Wire health monitoring
                if let Some(ref monitor) = state.channels.health_monitor {
                    let check_interval = std::time::Duration::from_secs(30);
                    let transport_timeout = std::time::Duration::from_secs(10);
                    monitor.monitor_channel_with_timeout(
                        name,
                        channel,
                        check_interval,
                        transport_timeout,
                    );
                }

                // Record snapshot
                if let Some(ref store) = state.channels.snapshot_store {
                    let snap = healthy_snapshot(name, None);
                    store.store(snap).await;
                }

                info!("WASM channel plugin '{}' initialized successfully", name);
            }
            Err(e) => {
                warn!("Failed to load WASM channel plugin '{}': {}", name, e);
            }
        }
    }

    Ok(())
}

/// Initialize a single channel by name and config.
pub(crate) async fn init_single_channel(
    state: Arc<GatewayState>,
    _config: &crate::gateway::config::GatewayConfig,
    name: &str,
    channel_config: &ChannelConfig,
) -> crate::Result<()> {
    info!("Initializing channel {} ({:?})", name, channel_config.channel_type);

    match channel_config.channel_type {
        ChannelType::Telegram => {
            #[cfg(feature = "telegram")]
            {
                init_telegram_channel(state.clone(), name, channel_config).await?;
            }
            #[cfg(not(feature = "telegram"))]
            {
                warn!("Telegram feature not enabled, skipping channel '{}'", name);
            }
        }
        ChannelType::Discord => {
            #[cfg(feature = "discord")]
            {
                init_discord_channel(state.clone(), name, channel_config).await?;
            }
            #[cfg(not(feature = "discord"))]
            {
                warn!("Discord feature not enabled, skipping channel '{}'", name);
            }
        }
        ChannelType::Slack => {
            #[cfg(feature = "slack")]
            {
                init_slack_channel(state.clone(), name, channel_config).await?;
            }
            #[cfg(not(feature = "slack"))]
            {
                warn!("Slack feature not enabled, skipping channel '{}'", name);
            }
        }
        ChannelType::Whatsapp => {
            #[cfg(feature = "whatsapp")]
            {
                init_whatsapp_channel(state.clone(), name, channel_config).await?;
            }
            #[cfg(not(feature = "whatsapp"))]
            {
                warn!("WhatsApp feature not enabled, skipping channel '{}'", name);
            }
        }
        ChannelType::Qq => {
            #[cfg(feature = "qq")]
            {
                init_qq_channel(state.clone(), name, channel_config).await?;
            }
            #[cfg(not(feature = "qq"))]
            {
                warn!("QQ feature not enabled, skipping channel '{}'", name);
            }
        }
        ChannelType::Feishu => {
            #[cfg(feature = "feishu")]
            {
                init_feishu_channel(state.clone(), name, channel_config).await?;
            }
            #[cfg(not(feature = "feishu"))]
            {
                warn!("Feishu/Lark feature not enabled, skipping channel '{}'", name);
            }
        }
        ChannelType::WebTerminal => {
            info!(
                "Channel '{}' (WebTerminal) uses Gateway WS/API directly, skipping adapter spawn",
                name
            );
        }
        ChannelType::Websocket => {
            info!("WebSocket channel '{}' requires external connection", name);
        }
        ChannelType::Signal => {
            #[cfg(feature = "signal")]
            {
                info!("Signal channel '{}' initialized (signal-cli daemon required)", name);
            }
            #[cfg(not(feature = "signal"))]
            {
                warn!("Signal feature not enabled, skipping channel '{}'", name);
            }
        }
        ChannelType::Imessage => {
            #[cfg(feature = "imessage")]
            {
                info!("iMessage channel '{}' initialized (BlueBubbles required)", name);
            }
            #[cfg(not(feature = "imessage"))]
            {
                warn!("iMessage feature not enabled, skipping channel '{}'", name);
            }
        }
        ChannelType::Webchat => {
            #[cfg(feature = "webchat")]
            {
                info!("WebChat channel '{}' initialized", name);
            }
            #[cfg(not(feature = "webchat"))]
            {
                warn!("WebChat feature not enabled, skipping channel '{}'", name);
            }
        }
    }

    // Record a healthy snapshot after successful channel initialization
    if let Some(ref store) = state.channels.snapshot_store {
        let snap = healthy_snapshot(name, None);
        store.store(snap).await;
    }

    // Start health monitoring if configured
    if let Some(ref monitor) = state.channels.health_monitor {
        let channels = state.channels.channels.read().await;
        if let Some(channel) = channels.get(name).cloned() {
            drop(channels);
            let check_interval = std::time::Duration::from_secs(30);
            monitor.monitor_channel(name, channel, check_interval);
            info!("Started health monitoring for channel '{}'", name);
        } else {
            warn!("Channel '{}' not found in registry for health monitoring", name);
        }
    }

    Ok(())
}

// ── Channel-specific inits ──────────────────────────────────────────

/// Initialize Telegram channel via ChannelExtension (skeleton alignment)
#[cfg(feature = "telegram")]
pub(crate) async fn init_telegram_channel(
    state: Arc<GatewayState>,
    name: &str,
    config: &ChannelConfig,
) -> crate::Result<()> {
    if let Some(token) = config.credentials.get("token") {
        let telegram_config = crate::channels::telegram::TelegramConfig::new(token)
            .allow_usernames(config.allow_from.clone());

        let channel = Arc::new(crate::channels::telegram::TelegramChannel::new(telegram_config));

        // Agent routing is now handled by the InboundPipeline (AgentRouter)
        let agent_name = config.agent_id.as_deref().unwrap_or("default");
        info!(
            "Telegram channel '{}' will route via InboundPipeline (default agent: '{}')",
            name, agent_name
        );

        // Set channel default so the router knows which agent to use for Telegram
        state
            .agents
            .router
            .set_channel_default(name, agent_name.to_string(), None)
            .await;

        // Create the channel extension
        let ext = Arc::new(crate::channels::TelegramChannelExtension::new(
            channel.clone(),
            state.channels.session_channels.clone(),
        ));

        // Create inbound channel: extension -> inbound pipeline
        let (inbound_tx, mut inbound_rx) = mpsc::channel::<crate::channels::IncomingMessage>(1000);

        // Spawn extension inbound task (Telegram bot -> inbound pipeline)
        let ext_inbound = ext.clone();
        tokio::spawn(async move {
            if let Err(e) = ext_inbound.run_inbound(inbound_tx).await {
                error!("Telegram extension inbound task failed: {}", e);
            }
        });

        // Bridge inbound messages into the unified entry channel
        let state_clone = state.clone();
        tokio::spawn(async move {
            while let Some(message) = inbound_rx.recv().await {
                if let Err(e) = state_clone.pipelines.inbound_entry.send(message).await {
                    warn!("Failed to submit Telegram message to inbound entry: {}", e);
                }
            }
        });

        // Create outbound channel: reply dispatcher -> extension outbound
        let (outbound_tx, outbound_rx) = mpsc::channel::<crate::channels::OutgoingMessage>(1000);

        // Spawn extension outbound task (outbound pipeline -> Telegram)
        let ext_outbound = ext.clone();
        tokio::spawn(async move {
            if let Err(e) = ext_outbound.run_outbound(outbound_rx).await {
                error!("Telegram extension outbound task failed: {}", e);
            }
        });

        // Register a bridge with the reply dispatcher so outbound pipeline
        // messages flow into the extension's run_outbound.
        let bridge = Arc::new(crate::channels::ChannelSenderBridge::new(name, outbound_tx));
        state
            .channels
            .reply_dispatcher
            .register_channel(name, bridge)
            .await;

        // Register extension in the extension registry
        register_channel_extension(&state, ext).await;

        // Keep the raw channel in the channels map for direct access
        state
            .channels
            .channels
            .write()
            .await
            .insert(name.to_string(), channel);
        info!("✅ Telegram channel '{}' initialized via ChannelExtension", name);
    } else {
        warn!("Telegram channel '{}' missing 'token' in credentials", name);
    }
    Ok(())
}

/// Initialize Discord channel
#[cfg(feature = "discord")]
pub(crate) async fn init_discord_channel(
    state: Arc<GatewayState>,
    name: &str,
    config: &ChannelConfig,
) -> crate::Result<()> {
    if let Some(token) = config.credentials.get("token") {
        // Create inbound bridge: Discord message_tx -> inbound pipeline
        let (inbound_tx, mut inbound_rx) =
            mpsc::unbounded_channel::<crate::channels::IncomingMessage>();
        let mut discord_config = crate::channels::discord::DiscordConfig::new(token);
        discord_config.message_tx = Some(inbound_tx);

        let channel = Arc::new(crate::channels::discord::DiscordChannel::new(discord_config));

        // Bridge inbound messages into the unified entry channel
        let state_clone = state.clone();
        tokio::spawn(async move {
            while let Some(msg) = inbound_rx.recv().await {
                if let Err(e) = state_clone.pipelines.inbound_entry.send(msg).await {
                    warn!("Failed to submit message to inbound entry: {}", e);
                }
            }
        });

        let channel_name = name.to_string();
        let channel_for_task = channel.clone();
        tokio::spawn(async move {
            if let Err(e) = channel_for_task.start().await {
                error!("Discord channel {} failed: {}", channel_name, e);
            }
        });
        state
            .channels
            .reply_dispatcher
            .register_channel(name, channel.clone())
            .await;
        state
            .channels
            .channels
            .write()
            .await
            .insert(name.to_string(), channel);
        info!("✅ Discord channel '{}' initialized", name);
    } else {
        warn!("Discord channel '{}' missing 'token' in credentials", name);
    }
    Ok(())
}

/// Initialize Slack channel
#[cfg(feature = "slack")]
pub(crate) async fn init_slack_channel(
    state: Arc<GatewayState>,
    name: &str,
    config: &ChannelConfig,
) -> crate::Result<()> {
    if let Some(token) = config.credentials.get("token") {
        // Create inbound bridge: Slack message_tx (Socket Mode) -> inbound pipeline
        let (inbound_tx, mut inbound_rx) =
            mpsc::unbounded_channel::<crate::channels::IncomingMessage>();
        let mut slack_config = crate::channels::slack::SlackConfig::new(token);
        slack_config.message_tx = Some(inbound_tx);
        if let Some(app_token) = config.credentials.get("app_token") {
            slack_config.app_token = Some(app_token.to_string());
        }

        let channel = Arc::new(crate::channels::slack::SlackChannel::new(slack_config));

        // Bridge inbound messages into the unified entry channel
        let state_clone = state.clone();
        tokio::spawn(async move {
            while let Some(msg) = inbound_rx.recv().await {
                if let Err(e) = state_clone.pipelines.inbound_entry.send(msg).await {
                    warn!("Failed to submit message to inbound entry: {}", e);
                }
            }
        });

        let channel_name = name.to_string();
        let channel_for_task = channel.clone();
        tokio::spawn(async move {
            if let Err(e) = channel_for_task.start().await {
                error!("Slack channel {} failed: {}", channel_name, e);
            }
        });
        state
            .channels
            .reply_dispatcher
            .register_channel(name, channel.clone())
            .await;
        state
            .channels
            .channels
            .write()
            .await
            .insert(name.to_string(), channel);
        info!("✅ Slack channel '{}' initialized", name);
    } else {
        warn!("Slack channel '{}' missing 'token' in credentials", name);
    }
    Ok(())
}

/// Initialize WhatsApp channel
#[cfg(feature = "whatsapp")]
pub(crate) async fn init_whatsapp_channel(
    state: Arc<GatewayState>,
    name: &str,
    config: &ChannelConfig,
) -> crate::Result<()> {
    if let (Some(phone_id), Some(token)) = (
        config.credentials.get("phone_number_id"),
        config.credentials.get("access_token"),
    ) {
        let whatsapp_config = crate::channels::whatsapp::WhatsappConfig::new(phone_id, token);

        let channel = Arc::new(crate::channels::whatsapp::WhatsappChannel::new(whatsapp_config));
        let channel_name = name.to_string();
        let channel_for_task = channel.clone();
        tokio::spawn(async move {
            if let Err(e) = channel_for_task.start().await {
                error!("WhatsApp channel {} failed: {}", channel_name, e);
            }
        });
        state
            .channels
            .reply_dispatcher
            .register_channel(name, channel.clone())
            .await;
        state
            .channels
            .channels
            .write()
            .await
            .insert(name.to_string(), channel);
        info!("✅ WhatsApp channel '{}' initialized", name);
    } else {
        warn!(
            "WhatsApp channel '{}' missing 'phone_number_id' or 'access_token' in credentials",
            name
        );
    }
    Ok(())
}

/// Initialize Feishu/Lark channel (outbound via ReplyDispatcher)
#[cfg(feature = "feishu")]
pub(crate) async fn init_feishu_channel(
    state: Arc<GatewayState>,
    name: &str,
    config: &ChannelConfig,
) -> crate::Result<()> {
    if let (Some(app_id), Some(app_secret)) =
        (config.credentials.get("app_id"), config.credentials.get("app_secret"))
    {
        let lark_config = crate::channels::lark::LarkConfig::new(app_id, app_secret);

        let channel = Arc::new(crate::channels::lark::LarkChannel::new(lark_config));
        let channel_name = name.to_string();
        let channel_for_task = channel.clone();
        tokio::spawn(async move {
            if let Err(e) = channel_for_task.start().await {
                error!("Feishu channel {} failed: {}", channel_name, e);
            }
        });
        state
            .channels
            .reply_dispatcher
            .register_channel(name, channel.clone())
            .await;
        state
            .channels
            .channels
            .write()
            .await
            .insert(name.to_string(), channel);
        info!("✅ Feishu channel '{}' initialized (inbound via webhook)", name);
    } else {
        warn!("Feishu channel '{}' missing 'app_id' or 'app_secret' in credentials", name);
    }
    Ok(())
}

/// Initialize QQ channel
#[cfg(feature = "qq")]
pub(crate) async fn init_qq_channel(
    state: Arc<GatewayState>,
    name: &str,
    config: &ChannelConfig,
) -> crate::Result<()> {
    if let (Some(app_id), Some(app_secret), Some(bot_qq)) = (
        config.credentials.get("app_id"),
        config.credentials.get("app_secret"),
        config.credentials.get("bot_qq"),
    ) {
        // Create inbound bridge: QQ WebSocket -> inbound pipeline
        let (inbound_tx, mut inbound_rx) =
            mpsc::unbounded_channel::<crate::channels::IncomingMessage>();
        let mut qq_config = crate::channels::qq::QqConfig::new(app_id, app_secret, bot_qq);
        qq_config.message_tx = Some(inbound_tx);

        let channel = Arc::new(crate::channels::qq::QqChannel::new(qq_config));

        // Bridge inbound messages into the unified entry channel
        let state_clone = state.clone();
        tokio::spawn(async move {
            while let Some(msg) = inbound_rx.recv().await {
                if let Err(e) = state_clone.pipelines.inbound_entry.send(msg).await {
                    warn!("Failed to submit message to inbound entry: {}", e);
                }
            }
        });

        let channel_name = name.to_string();
        let channel_for_task = channel.clone();
        tokio::spawn(async move {
            if let Err(e) = channel_for_task.start().await {
                error!("QQ channel {} failed: {}", channel_name, e);
            }
        });
        state
            .channels
            .reply_dispatcher
            .register_channel(name, channel.clone())
            .await;
        state
            .channels
            .channels
            .write()
            .await
            .insert(name.to_string(), channel);
        info!("✅ QQ channel '{}' initialized", name);
    } else {
        warn!(
            "QQ channel '{}' missing required credentials (app_id, app_secret, bot_qq)",
            name
        );
    }
    Ok(())
}

/// Register a channel extension in the extension registry.
async fn register_channel_extension(state: &GatewayState, ext: Arc<dyn ChannelExtension>) {
    let mut registry = state.channels.extensions.write().await;
    registry.register(ext);
}
