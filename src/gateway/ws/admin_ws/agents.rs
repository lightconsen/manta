//! WS admin handlers: agents.

use std::sync::Arc;

use super::super::{parse_params, WsRequest, WsResponse};
use crate::gateway::GatewayState;

// ── Agents ──────────────────────────────────────────────────────────────

/// `agents.create` — create a new agent personality (`{ name, description, ... }`).
pub(crate) async fn handle_agents_create(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let config: crate::agent::AgentConfig = match parse_params(req) {
        Ok(c) => c,
        Err(res) => return res,
    };
    let agent_id = format!("agent-{}", uuid::Uuid::new_v4());
    match crate::gateway::agent_spawn::spawn_agent_inner(
        state.clone(),
        agent_id.clone(),
        config.clone(),
    )
    .await
    {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "id": agent_id,
                "status": "created",
                "config": {
                    "max_context_tokens": config.max_context_tokens,
                    "max_concurrent_tools": config.max_concurrent_tools,
                    "temperature": config.temperature,
                    "max_tokens": config.max_tokens,
                },
            }),
        ),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", &format!("Failed to create agent: {}", e)),
    }
}

/// `agents.delete` — delete an agent (`{ id }`).
pub(crate) async fn handle_agents_delete(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let agent_exists = {
        let agents = state.agents.agents.read().await;
        agents.contains_key(&id)
    };
    if !agent_exists {
        return WsResponse::err(&req.id, "NOT_FOUND", "agent not found");
    }
    let tx = {
        let agents = state.agents.agents.read().await;
        agents.get(&id).map(|h| h.tx.clone())
    };
    if let Some(tx) = tx {
        if let Err(e) = tx
            .send(crate::gateway::runtime::AgentCommand::Shutdown)
            .await
        {
            tracing::warn!("Failed to send shutdown to agent {}: {}", id, e);
        }
    }
    {
        let mut agents = state.agents.agents.write().await;
        agents.remove(&id);
    }
    if let Err(e) = state
        .events
        .tx
        .send(crate::gateway::GatewayEvent::AgentStatus {
            agent_id: id.clone(),
            status: crate::gateway::AgentStatus::Shutdown,
        })
    {
        tracing::warn!("Failed to broadcast agent shutdown event for {}: {}", id, e);
    }
    WsResponse::ok(&req.id, serde_json::json!({ "id": id, "status": "deleted" }))
}
