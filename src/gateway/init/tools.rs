//! Tool subsystem initialization.
//!
//! Creates the tool registry, MCP manager, approval queue, canvas manager,
//! plugin manager, channel registry, and computer adapter.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::session_store::SessionStore;
use crate::canvas::CanvasManager;
use crate::channels::{Channel, ChannelExtensionRegistry, IncomingMessage};
use crate::computer::ComputerAdapter;
use crate::gateway::GatewayConfig;
use crate::memory::MemoryManager;
use crate::model_router::ModelRouter;
use crate::plugins::PluginManager;
use crate::security::content_filter::ContentFilter;
use crate::security::runtime_audit::AuditLogger;
use crate::tools::approval::ApprovalQueue;
use crate::tools::mcp::{McpEvent, McpManager};
use crate::tools::ToolRegistry;

/// Tool subsystem initialization result.
pub struct ToolsInit {
    pub mcp_manager: Arc<McpManager>,
    pub mcp_event_rx: mpsc::UnboundedReceiver<McpEvent>,
    pub approval_queue: Arc<ApprovalQueue>,
    pub memory_manager_holder: Arc<RwLock<Option<Arc<MemoryManager>>>>,
    pub tool_registry: Arc<ToolRegistry>,
    pub computer_adapter: Option<Arc<dyn ComputerAdapter>>,
    pub plugin_manager: Arc<PluginManager>,
    pub canvas_manager: Arc<CanvasManager>,
    pub channels: Arc<RwLock<HashMap<String, Arc<dyn Channel>>>>,
    pub channel_extensions: Arc<RwLock<ChannelExtensionRegistry>>,
}

/// Initialize MCP manager with its internal event channel.
pub async fn init_mcp_manager() -> (Arc<McpManager>, mpsc::UnboundedReceiver<McpEvent>) {
    let (mcp_event_tx, mcp_event_rx) = mpsc::unbounded_channel::<McpEvent>();
    let mcp_manager = Arc::new(McpManager::new().with_event_tx(mcp_event_tx).await);
    (mcp_manager, mcp_event_rx)
}

/// Initialize the computer / desktop automation adapter.
pub async fn init_computer_adapter(
    config: &GatewayConfig,
    tool_registry: Arc<ToolRegistry>,
) -> Option<Arc<dyn ComputerAdapter>> {
    if !config.computer.enabled {
        return None;
    }

    if let Some(ref host) = config.computer.remote_control.host {
        let rc_config = crate::computer::RemoteControlConfig {
            host: host.clone(),
            user: config
                .computer
                .remote_control
                .user
                .clone()
                .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "user".to_string())),
            port: config.computer.remote_control.port,
            protocol: crate::computer::RemoteProtocol::Ssh {
                key_path: config.computer.remote_control.key_path.clone(),
            },
            display: config.computer.remote_control.display.clone(),
            ssh_extra_args: config.computer.remote_control.ssh_extra_args.clone(),
            connect_timeout: std::time::Duration::from_secs(
                config.computer.remote_control.timeout_secs,
            ),
        };
        match crate::computer::RemoteControlAdapter::new(rc_config, tool_registry.clone()).await {
            Ok(adapter) => {
                info!("Remote control adapter connected to {} for desktop automation", host);
                return Some(Arc::new(adapter));
            }
            Err(e) => {
                warn!(
                    "Failed to connect remote control adapter to {}: {}. Falling back to local \
                     adapter.",
                    host, e
                );
            }
        }
    }

    if crate::computer::has_display_server() {
        match crate::computer::create_adapter(tool_registry.clone()).await {
            Ok(adapter) => {
                info!("Computer adapter initialized for desktop automation");
                Some(Arc::from(adapter))
            }
            Err(e) => {
                warn!("Failed to initialize computer adapter: {}", e);
                None
            }
        }
    } else {
        warn!(
            "No display server detected and no remote_control host configured; desktop automation \
             disabled"
        );
        None
    }
}

/// Initialize the plugin manager and wire provider callbacks.
pub async fn init_plugin_manager(
    _config: &GatewayConfig,
    tool_registry: Arc<ToolRegistry>,
    model_router: Arc<ModelRouter>,
    channels: Arc<RwLock<HashMap<String, Arc<dyn Channel>>>>,
) -> crate::Result<Arc<PluginManager>> {
    let plugins_dir = crate::dirs::config_dir().join("plugins");
    let plugin_manager = {
        let pm = PluginManager::new(plugins_dir).await?;
        pm.set_tool_registry(tool_registry).await;
        Arc::new(pm)
    };

    // Wire plugin manager to register plugin-backed providers with the model router
    {
        let mr_register = model_router.clone();
        let mr_unregister = model_router.clone();
        plugin_manager
            .set_provider_callbacks(
                Arc::new(move |name: String, provider: Arc<dyn crate::providers::Provider + Send + Sync>| {
                    let mr = mr_register.clone();
                    tokio::spawn(async move {
                        if let Err(e) = mr.add_provider_instance(&name, provider).await {
                            warn!("Failed to register plugin provider '{}': {}", name, e);
                        }
                    });
                }),
                Arc::new(move |name: String| {
                    let mr = mr_unregister.clone();
                    tokio::spawn(async move {
                        if let Err(e) = mr.remove_provider(&name).await {
                            warn!("Failed to unregister plugin provider '{}': {}", name, e);
                        }
                    });
                }),
            )
            .await;
    }

    // Wire plugin manager channel callbacks
    #[cfg(feature = "plugins")]
    {
        let (plugin_inbound_tx, _plugin_inbound_rx) = mpsc::unbounded_channel::<IncomingMessage>();

        let channels_reg = channels.clone();
        let channels_unreg = channels.clone();
        plugin_manager
            .set_channel_callbacks(
                Arc::new(
                    move |name: String, channel: Arc<dyn crate::channels::Channel + Send + Sync>| {
                        let ch = channels_reg.clone();
                        tokio::spawn(async move {
                            ch.write().await.insert(name.clone(), channel);
                            info!("Registered plugin channel '{}'", name);
                        });
                    },
                ),
                Arc::new(move |name: String| {
                    let ch = channels_unreg.clone();
                    tokio::spawn(async move {
                        ch.write().await.remove(&name);
                        info!("Deregistered plugin channel '{}'", name);
                    });
                }),
            )
            .await;

        plugin_manager
            .set_channel_message_tx(plugin_inbound_tx)
            .await;
    }

    Ok(plugin_manager)
}

/// Initialize the full tool subsystem.
pub async fn init_tools(
    config: &GatewayConfig,
    acp: Arc<AcpControlPlane>,
    session_store: Option<Arc<SessionStore>>,
    audit_log_dyn: Arc<dyn AuditLogger>,
    model_router: Arc<ModelRouter>,
) -> crate::Result<ToolsInit> {
    let (mcp_manager, mcp_event_rx) = init_mcp_manager().await;
    let approval_queue = Arc::new(ApprovalQueue::new());
    let memory_manager_holder: Arc<RwLock<Option<Arc<MemoryManager>>>> =
        Arc::new(RwLock::new(None));

    let tool_registry = Arc::new(
        crate::gateway::create_default_tool_registry(
            acp.clone(),
            mcp_manager.clone(),
            approval_queue.clone(),
            session_store.clone(),
            memory_manager_holder.clone(),
            config.capabilities.clone(),
            audit_log_dyn,
            Some(Arc::new(ContentFilter::default())),
        )
        .await?,
    );

    let computer_adapter = init_computer_adapter(config, tool_registry.clone()).await;
    let channels = Arc::new(RwLock::new(HashMap::<String, Arc<dyn Channel>>::new()));
    let plugin_manager =
        init_plugin_manager(config, tool_registry.clone(), model_router, channels.clone()).await?;
    let canvas_manager = Arc::new(CanvasManager::new());
    let channel_extensions = Arc::new(RwLock::new(ChannelExtensionRegistry::new()));

    Ok(ToolsInit {
        mcp_manager,
        mcp_event_rx,
        approval_queue,
        memory_manager_holder,
        tool_registry,
        computer_adapter,
        plugin_manager,
        canvas_manager,
        channels,
        channel_extensions,
    })
}
