//! Agent subsystem initialization.
//!
//! Builds the ACP control plane, model router, skill manager, agent registry,
//! and session manager that make up the agent runtime.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::session_store::SessionStore;
use crate::agent::{AgentBuilder, AgentConfig, AgentRegistry, SessionManager};
use crate::gateway::GatewayConfig;
use crate::model_router::{ModelAlias, ModelRouter, ModelRouterConfig};
use crate::skills::SkillManager;
use crate::tools::ToolRegistry;

/// Agent subsystem initialization result.
pub struct AgentsInit {
    pub acp: Arc<AcpControlPlane>,
    pub model_router: Arc<ModelRouter>,
    pub skills_manager: Arc<RwLock<SkillManager>>,
    pub agent_registry: Arc<RwLock<AgentRegistry>>,
    pub session_manager: Arc<RwLock<SessionManager>>,
}

/// Initialize the ACP control plane.
pub async fn init_acp(
    config: &GatewayConfig,
    session_store: Option<Arc<SessionStore>>,
) -> Arc<AcpControlPlane> {
    let acp_max_iter = config.acp.max_iterations;
    let acp = if let Some(ref store) = session_store {
        info!("Wiring ACP control plane to SessionStore for persistent subagent sessions");
        Arc::new(AcpControlPlane::new(acp_max_iter).with_store(store.clone()))
    } else {
        if config.storage.storage_type == "sqlite" {
            warn!(
                "Storage type is 'database' but no SessionStore is available; \
                 ACP subagent sessions will not persist. Check the SQLite pool configuration."
            );
        } else {
            info!("ACP control plane running without SessionStore (ephemeral subagent sessions)");
        }
        Arc::new(AcpControlPlane::new(acp_max_iter))
    };
    acp.load_persisted_sessions().await;
    acp
}

/// Initialize the model router and configure default aliases.
pub async fn init_model_router(config: &GatewayConfig) -> Arc<ModelRouter> {
    let mut model_router_config = ModelRouterConfig::default();

    if let Some(first_provider) = config.providers.keys().next() {
        let alias = ModelAlias {
            name: "default".to_string(),
            provider: first_provider.clone(),
            model: config.model.clone(),
            temperature: None,
            max_tokens: None,
        };
        model_router_config
            .aliases
            .insert("default".to_string(), alias);
        model_router_config.default_model = "default".to_string();
    }

    let model_router = Arc::new(ModelRouter::new(model_router_config));
    for (name, provider_config) in &config.providers {
        info!("Configuring provider: {}", name);
        if let Err(e) = model_router
            .add_provider(name, provider_config.clone())
            .await
        {
            warn!("Failed to add provider '{}': {}", name, e);
        }
    }

    model_router
}

/// Configure the ACP default agent builder after provider and tools are ready.
pub async fn configure_acp_agent_builder(
    acp: &AcpControlPlane,
    config: &GatewayConfig,
    model_router: Arc<ModelRouter>,
    tool_registry: Arc<ToolRegistry>,
    skills_manager: Arc<RwLock<SkillManager>>,
) {
    if let Ok(default_provider) = model_router.create_default_provider().await {
        let mut default_agent_config = config.default_agent.clone();
        default_agent_config.workspace_dir = config
            .workspace_dir
            .as_ref()
            .map(crate::dirs::resolve_tilde);
        default_agent_config.workspace_only = config.workspace_only;
        let provider_clone = default_provider.clone();
        let model_router_clone = model_router.clone();
        let default_model = config.model.clone();
        let skills_manager_clone = Arc::clone(&skills_manager);
        acp.set_agent_builder(move || {
            AgentBuilder::new()
                .config(default_agent_config.clone())
                .provider(provider_clone.clone())
                .tools(tool_registry.clone())
                .model_router(model_router_clone.clone())
                .model_alias(default_model.clone())
                .planner_model(default_model.clone())
                .skill_manager(Arc::clone(&skills_manager_clone))
                .build()
        })
        .await;
    } else {
        warn!(
            "No default LLM provider available — ACP subagent spawning will fail until a provider is configured"
        );
    }
}

/// Initialize skill manager, agent registry, and session manager.
pub async fn init_agent_state() -> crate::Result<(
    Arc<RwLock<SkillManager>>,
    Arc<RwLock<AgentRegistry>>,
    Arc<RwLock<SessionManager>>,
)> {
    let skills_manager = Arc::new(RwLock::new(SkillManager::new().await?));
    let agent_registry = Arc::new(RwLock::new(AgentRegistry::new()));
    let session_manager = Arc::new(RwLock::new(SessionManager::new()));
    Ok((skills_manager, agent_registry, session_manager))
}

/// Convenience helper that wires the ACP builder and returns the agent subsystem bundle.
#[allow(clippy::too_many_arguments)]
pub async fn init_agents(
    config: &GatewayConfig,
    session_store: Option<Arc<SessionStore>>,
    tool_registry: Arc<ToolRegistry>,
) -> crate::Result<AgentsInit> {
    let acp = init_acp(config, session_store).await;
    let model_router = init_model_router(config).await;
    let (skills_manager, agent_registry, session_manager) = init_agent_state().await?;

    configure_acp_agent_builder(
        &acp,
        config,
        model_router.clone(),
        tool_registry,
        skills_manager.clone(),
    )
    .await;

    Ok(AgentsInit {
        acp,
        model_router,
        skills_manager,
        agent_registry,
        session_manager,
    })
}
