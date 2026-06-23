//! Gateway-side extension trait that lets an [`AgentHandle`] spawn ACP
//! subagents.
//!
//! Keeping the trait in the gateway module (rather than in `acp`) avoids a
//! dependency cycle: `acp` exposes the control plane and the trait only uses
//! public ACP types, while the implementation is defined for the gateway's
//! `AgentHandle`.

use async_trait::async_trait;

use crate::acp::config::{AcpSessionId, SubagentConfig};
use crate::acp::control_plane::AcpControlPlane;
use crate::acp::subagent::SubagentHandle;
use crate::gateway::AgentHandle;

/// Public handle extension trait for spawning ACP subagents from an agent.
#[async_trait]
pub trait AcpAgentExt {
    /// Spawn a subagent from this agent.
    async fn spawn_subagent(
        &self,
        acp: &AcpControlPlane,
        config: SubagentConfig,
    ) -> crate::Result<SubagentHandle>;
}

#[async_trait]
impl AcpAgentExt for AgentHandle {
    async fn spawn_subagent(
        &self,
        acp: &AcpControlPlane,
        config: SubagentConfig,
    ) -> crate::Result<SubagentHandle> {
        let session_id = AcpSessionId::new();
        acp.spawn_subagent(session_id, self.id.clone(), config)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::config::{SpawnMode, ThreadBinding};

    #[tokio::test]
    async fn test_acp_agent_ext_spawns_subagent() {
        let provider = std::sync::Arc::new(
            crate::providers::mock::MockProvider::new()
                .with_responses(vec![crate::providers::Message::assistant("mock response")]),
        );
        let tools = std::sync::Arc::new(crate::tools::ToolRegistry::new());
        let agent_config = crate::agent::AgentConfig::default();
        let agent = crate::agent::Agent::new(agent_config.clone(), provider, tools);
        let acp = AcpControlPlane::new(50).with_agent_builder(move || {
            let provider = std::sync::Arc::new(
                crate::providers::mock::MockProvider::new()
                    .with_responses(vec![crate::providers::Message::assistant("mock response")]),
            );
            let tools = std::sync::Arc::new(crate::tools::ToolRegistry::new());
            Ok(crate::agent::Agent::new(crate::agent::AgentConfig::default(), provider, tools))
        });
        let handle = AgentHandle {
            id: "parent-123".to_string(),
            config: agent_config,
            tx: tokio::sync::mpsc::channel(1).0,
            query_tx: tokio::sync::mpsc::channel(1).0,
            busy: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            agent: std::sync::Arc::new(agent),
        };

        let _ = acp.create_session(handle.id.clone()).await;
        let result = handle
            .spawn_subagent(
                &acp,
                SubagentConfig {
                    mode: SpawnMode::Run,
                    thread_binding: ThreadBinding::New,
                    ..SubagentConfig::default()
                },
            )
            .await;

        assert!(
            result.is_ok(),
            "spawn_subagent should succeed with a valid session: {:?}",
            result.err()
        );
        let subagent = result.unwrap();
        assert_eq!(subagent.parent_id, handle.id);
        assert!(!subagent.session_id.0.is_empty());
    }
}
