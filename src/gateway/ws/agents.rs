//! Agent list/get/registry, health, system presence.

use super::*;
pub(super) async fn handle_agents_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agents = {
        let agents = state.agents.agents.read().await;
        agents.keys().cloned().collect::<Vec<_>>()
    };

    WsResponse::ok(&req.id, serde_json::json!({ "agents": agents }))
}

pub(super) async fn handle_agents_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct GetParams {
        agent_id: String,
    }

    let params: GetParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let agent = {
        let agents = state.agents.agents.read().await;
        agents.get(&params.agent_id).cloned()
    };

    let personality = {
        let registry = state.agents.registry.read().await;
        registry.get(&params.agent_id).cloned()
    };

    match agent {
        Some(handle) => {
            let cfg = &handle.config;
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "agent_id": params.agent_id,
                    "busy": handle.busy.load(std::sync::atomic::Ordering::Acquire),
                    "status": if handle.busy.load(std::sync::atomic::Ordering::Acquire) { "busy" } else { "idle" },
                    "config": {
                        "temperature": cfg.temperature,
                        "max_tokens": cfg.max_tokens,
                        "max_turns": cfg.max_turns,
                        "max_concurrent_tools": cfg.max_concurrent_tools,
                        "workspace_only": cfg.workspace_only,
                        "compaction_model": cfg.compaction_model,
                        "system_prompt": cfg.system_prompt,
                    },
                    "personality": personality.map(|p| serde_json::json!({
                        "display_name": p.display_name(),
                        "is_valid": p.is_valid,
                        "has_heartbeat": !p.heartbeat.is_empty(),
                        "has_soul": !p.soul.is_empty(),
                        "has_identity": !p.identity.is_empty(),
                        "has_memory": !p.memory.is_empty(),
                    })),
                }),
            )
        }
        None => {
            // Agent not spawned but may have a personality on disk
            if let Some(p) = personality {
                let cfg = p.to_agent_config();
                let config_json = match serde_json::to_value(&cfg) {
                    Ok(v) => v,
                    Err(e) => {
                        return WsResponse::err(
                            &req.id,
                            "SERIALIZE_FAILED",
                            format!("Failed to serialize agent config: {}", e),
                        );
                    }
                };
                WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "agent_id": params.agent_id,
                        "busy": false,
                        "status": "stopped",
                        "config": config_json,
                        "personality": {
                            "display_name": p.display_name(),
                            "is_valid": p.is_valid,
                            "has_heartbeat": !p.heartbeat.is_empty(),
                            "has_soul": !p.soul.is_empty(),
                            "has_identity": !p.identity.is_empty(),
                            "has_memory": !p.memory.is_empty(),
                        },
                    }),
                )
            } else {
                error_agent_not_found(&req.id)
            }
        }
    }
}

pub(super) async fn handle_agents_registry(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let registry = state.agents.registry.read().await;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entries: Vec<serde_json::Value> = Vec::new();

    // 1. Registry-discovered agents from disk
    for id in registry.list() {
        if let Some(p) = registry.get(&id) {
            seen.insert(id.clone());
            entries.push(serde_json::json!({
                "id": p.id,
                "display_name": p.display_name(),
                "emoji": p.emoji(),
                "is_valid": p.is_valid,
                "has_heartbeat": !p.heartbeat.is_empty(),
            }));
        }
    }

    // 2. Runtime-spawned agents not in registry (e.g. default)
    {
        let agents = state.agents.agents.read().await;
        for id in agents.keys() {
            if !seen.contains(id) {
                entries.push(serde_json::json!({
                    "id": id,
                    "display_name": id.as_str(),
                    "emoji": "🤖",
                    "is_valid": true,
                    "has_heartbeat": false,
                }));
            }
        }
    }

    WsResponse::ok(&req.id, serde_json::json!({ "agents": entries, "count": entries.len() }))
}

pub(super) async fn handle_health(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agent_count = {
        let agents = state.agents.agents.read().await;
        agents.len()
    };

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "healthy",
            "agents": agent_count,
            "protocol_version": PROTOCOL_VERSION,
        }),
    )
}

pub(super) async fn handle_system_presence(req: &WsRequest) -> WsResponse {
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "online": true,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;

    fn req(id: &str, params: Option<serde_json::Value>) -> WsRequest {
        WsRequest {
            frame_type: "req".into(),
            id: id.into(),
            method: "x".into(),
            params,
        }
    }

    async fn state() -> Arc<GatewayState> {
        Arc::new(make_test_state(GatewayConfig::default()).await)
    }

    #[tokio::test]
    async fn agents_list_empty_ok() {
        let state = state().await;
        let resp = handle_agents_list(&req("r1", None), &state).await;
        assert!(resp.ok);
        let agents = resp.payload.as_ref().unwrap()["agents"].as_array().unwrap();
        assert!(agents.is_empty(), "fresh state has no agents");
    }

    #[tokio::test]
    async fn agents_get_missing_params_errors() {
        let state = state().await;
        let resp = handle_agents_get(&req("r1", None), &state).await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn agents_get_unknown_agent_not_found() {
        let state = state().await;
        let params = Some(serde_json::json!({ "agent_id": "ghost" }));
        let resp = handle_agents_get(&req("r1", params), &state).await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "AGENT_NOT_FOUND");
    }

    #[tokio::test]
    async fn agents_get_invalid_params_errors() {
        let state = state().await;
        let resp =
            handle_agents_get(&req("r1", Some(serde_json::json!({ "nope": 1 }))), &state).await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn agents_registry_empty_ok() {
        let state = state().await;
        let resp = handle_agents_registry(&req("r1", None), &state).await;
        assert!(resp.ok);
        let payload = resp.payload.as_ref().unwrap();
        assert_eq!(payload["count"], 0);
        assert!(payload["agents"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn health_reports_healthy_zero_agents() {
        let state = state().await;
        let resp = handle_health(&req("r1", None), &state).await;
        assert!(resp.ok);
        let payload = resp.payload.as_ref().unwrap();
        assert_eq!(payload["status"], "healthy");
        assert_eq!(payload["agents"], 0);
        assert!(payload["protocol_version"].is_number());
    }

    #[tokio::test]
    async fn system_presence_online() {
        let resp = handle_system_presence(&req("r1", None)).await;
        assert!(resp.ok);
        assert_eq!(resp.payload.as_ref().unwrap()["online"], true);
    }
}
