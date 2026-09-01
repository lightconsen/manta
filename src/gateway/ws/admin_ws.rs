//! WebSocket RPC handlers for admin-style operations (plugins, providers,
//! updates, cloud, onboarding, connectors catalog, device pairing, channels,
//! system reload, status). These let the built-in UI and CLI drive everything
//! over WS (single transport, no CORS). The remaining REST surface is only
//! where HTTP is required: OpenAI compatibility, OAuth login, webhooks,
//! artifact/file downloads, and health/metrics probes.

use std::sync::Arc;

use serde::Deserialize;

use super::{parse_params, WsRequest, WsResponse};
use crate::gateway::GatewayState;

/// Error helper for the common "cloud not enabled / not signed in" case.
fn cloud_unavailable(req: &WsRequest) -> WsResponse {
    WsResponse::err(&req.id, "UNAUTHORIZED", "cloud not enabled or not signed in")
}

// ── Onboarding ──────────────────────────────────────────────────────────

/// `onboarding.status` — `{ "status": "pending" | "done" }`.
pub(super) async fn handle_onboarding_status(
    req: &WsRequest,
    _state: &Arc<GatewayState>,
) -> WsResponse {
    let dir = crate::dirs::workspace_data_dir();
    match crate::memory::onboarding::status(&dir).await {
        Ok(crate::memory::onboarding::OnboardingStatus::Done) => {
            WsResponse::ok(&req.id, serde_json::json!({ "status": "done" }))
        }
        Ok(crate::memory::onboarding::OnboardingStatus::Pending) => {
            WsResponse::ok(&req.id, serde_json::json!({ "status": "pending" }))
        }
        Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
    }
}

/// `onboarding.apply` — `{ ok: true }` on success.
pub(super) async fn handle_onboarding_apply(
    req: &WsRequest,
    _state: &Arc<GatewayState>,
) -> WsResponse {
    let payload: crate::memory::onboarding::OnboardingPayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let dir = crate::dirs::workspace_data_dir();
    match crate::memory::onboarding::apply(&dir, &payload).await {
        Ok(()) => WsResponse::ok(&req.id, serde_json::json!({ "ok": true })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
    }
}

// ── System reload / channels ────────────────────────────────────────────

/// `system.reload` — reload plugins/config/providers/MCP/skills without a
/// restart. `{ scope }` is "all" by default (also "plugins" | "config" |
/// "providers" | "mcp" | "skills" | "channels"). Mirrors the former
/// `POST /api/v1/reload`.
pub(super) async fn handle_system_reload(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        #[serde(default = "default_reload_scope")]
        scope: String,
    }
    fn default_reload_scope() -> String {
        "all".to_string()
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let result = crate::gateway::handlers::admin::run_reload(state, &p.scope).await;
    WsResponse::ok(&req.id, result)
}

/// `channels.list` — all configured channels and their enabled state.
pub(super) async fn handle_channels_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let config = state.config.read().await;
    let channels: Vec<serde_json::Value> = config
        .channels
        .iter()
        .map(|(name, cfg)| {
            serde_json::json!({
                "name": name,
                "type": cfg.channel_type,
                "enabled": cfg.enabled,
                "agent_id": cfg.agent_id,
            })
        })
        .collect();
    WsResponse::ok(&req.id, serde_json::json!({ "channels": channels }))
}

/// `channels.enable` — enable a channel (`{ name }`), persisting to config.
pub(super) async fn handle_channels_enable(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let name = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["name"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    if name.is_empty() {
        return WsResponse::err(&req.id, "INVALID_PARAMS", "missing channel name");
    }
    {
        let mut config_guard = state.config.write().await;
        let config = Arc::make_mut(&mut config_guard);
        let Some(channel_config) = config.channels.get_mut(&name) else {
            return WsResponse::err(&req.id, "NOT_FOUND", &format!("Channel '{}' not found", name));
        };
        if channel_config.enabled {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "name": name,
                    "enabled": true,
                    "message": "Channel is already enabled",
                }),
            );
        }
        channel_config.enabled = true;
    }
    if let Err(res) = super::persist_config(state).await {
        return res;
    }
    tracing::info!("Enabled channel '{}' via WS", name);
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "name": name, "enabled": true, "message": "Channel enabled" }),
    )
}

/// `channels.disable` — disable a channel (`{ name }`), persisting to config.
pub(super) async fn handle_channels_disable(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let name = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["name"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    if name.is_empty() {
        return WsResponse::err(&req.id, "INVALID_PARAMS", "missing channel name");
    }
    {
        let mut config_guard = state.config.write().await;
        let config = Arc::make_mut(&mut config_guard);
        let Some(channel_config) = config.channels.get_mut(&name) else {
            return WsResponse::err(&req.id, "NOT_FOUND", &format!("Channel '{}' not found", name));
        };
        if !channel_config.enabled {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "name": name,
                    "enabled": false,
                    "message": "Channel is already disabled",
                }),
            );
        }
        channel_config.enabled = false;
    }
    if let Err(res) = super::persist_config(state).await {
        return res;
    }
    tracing::info!("Disabled channel '{}' via WS", name);
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "name": name, "enabled": false, "message": "Channel disabled" }),
    )
}

// ── Connectors catalog (marketplace) ────────────────────────────────────

/// `connectors.catalog` — the marketplace catalog (cached) joined with each
/// entry's installed state.
///
/// On a first visit with an empty cache the handler one-shot syncs from the
/// cloud catalog URL when cloud mode is active (feature + `cloud.enabled` +
/// logged in), so member/cloud entries are visible immediately. Returns
/// `{ version, synced, entries }` — the shape the UI consumes.
pub(super) async fn handle_connectors_catalog(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let manager = &state.tools.connector_manager;
    let doc: Option<crate::mcp::connectors::catalog::CatalogDocument> = {
        let cached = match manager.cached_catalog().await {
            Ok(Some(doc)) => Some(doc),
            Ok(None) => None,
            Err(e) => return WsResponse::err(&req.id, "INTERNAL", e.to_string()),
        };
        #[cfg(feature = "cloud")]
        let synced = if cached.is_none() {
            let cfg = state.config.read().await.cloud.clone();
            if cfg.enabled && crate::cloud::session::logged_in().await {
                let url = format!("{}/catalog.json", cfg.api_base.trim_end_matches('/'));
                if let Ok((fresh, _)) = manager.sync_catalog(&url).await {
                    Some(fresh)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        #[cfg(not(feature = "cloud"))]
        let synced: Option<crate::mcp::connectors::catalog::CatalogDocument> = None;
        cached.or(synced)
    };

    let installed = manager.list().await.unwrap_or_default();
    let installed_by_id: std::collections::HashMap<String, _> =
        installed.into_iter().map(|s| (s.id.clone(), s)).collect();

    let entries: Vec<_> = doc
        .iter()
        .flat_map(|d| d.connectors.iter())
        .map(|e| {
            let inst = installed_by_id.get(&e.id);
            // Experts are "installed" once their role dir exists in agents/.
            let expert_installed =
                e.entry_type == "expert" && crate::dirs::agents_dir().join(&e.id).is_dir();
            serde_json::json!({
                "id": e.id,
                "version": e.version,
                "display_name": e.display_name,
                "description": e.description,
                "icon": e.icon,
                "type": e.entry_type,
                "kind": e.kind,
                "visibility": e.visibility,
                "credits_per_use": e.credits_per_use,
                "category": e.category,
                "installed": inst.is_some() || expert_installed,
                "installed_version": inst.map(|s| s.version.clone()),
                "state": inst.map(|s| serde_json::to_value(s.state).unwrap_or_default()),
            })
        })
        .collect();

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "version": doc.as_ref().map(|d| d.version).unwrap_or(0),
            "synced": doc.is_some(),
            "entries": entries,
        }),
    )
}

/// `connectors.catalog_install` — install a catalog entry (connector,
/// skill, or expert role).
pub(super) async fn handle_connectors_catalog_install(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let manager = &state.tools.connector_manager;
    let Some(doc) = manager.cached_catalog().await.ok().flatten() else {
        return WsResponse::err(
            &req.id,
            "INTERNAL",
            "marketplace catalog is empty — sync it first",
        );
    };
    let Some(entry) = doc
        .connectors
        .into_iter()
        .filter(|e| e.id == p.id)
        .max_by_key(|e| crate::mcp::connectors::catalog::catalog_version_key(&e.version))
    else {
        return WsResponse::err(&req.id, "NOT_FOUND", format!("no catalog entry for '{}'", p.id));
    };

    if entry.entry_type == "expert" {
        match manager.install_expert(&entry).await {
            Ok(agents) => {
                let _ = state.agents.registry.write().await.discover().await;
                WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "id": entry.id, "type": "expert", "agents": agents, "installed": true }),
                )
            }
            Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
        }
    } else {
        match manager.upgrade(&entry).await {
            Ok(summary) => {
                WsResponse::ok(&req.id, serde_json::to_value(&summary).unwrap_or_default())
            }
            Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
        }
    }
}

// ── Turn traces (replay) ────────────────────────────────────────────────

/// `traces.get` — replay a recorded agent turn (`{ turn_id }`). Returns the
/// turn summary + full event list. Formerly `GET /api/traces/:turn_id`.
pub(super) async fn handle_traces_get(req: &WsRequest, _state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        turn_id: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    if p.turn_id.is_empty()
        || p.turn_id.contains('/')
        || p.turn_id.contains('\\')
        || p.turn_id.contains("..")
    {
        return WsResponse::err(&req.id, "INVALID_PARAMS", "invalid turn_id");
    }

    // Turn dirs are `turns/YYYY-MM-DD/<turn_id>/`; the date isn't in the id, so
    // scan the date partitions (bounded — a local personal-assistant store).
    let base = crate::dirs::turns_dir();
    let mut turn_dir = None;
    if let Ok(rd) = std::fs::read_dir(&base) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join(&p.turn_id).is_dir() {
                turn_dir = Some(path.join(&p.turn_id));
                break;
            }
        }
    }
    let Some(dir) = turn_dir else {
        return WsResponse::err(&req.id, "NOT_FOUND", "turn not found");
    };

    let summary = std::fs::read_to_string(dir.join("summary.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let full_events = std::fs::read_to_string(dir.join("full.json"))
        .ok()
        .map(|s| {
            s.lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .collect::<Vec<_>>()
        });

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "turn_id": p.turn_id,
            "summary": summary,
            "full_trace": full_events,
        }),
    )
}

// ── Agents ──────────────────────────────────────────────────────────────

/// `agents.create` — create a new agent personality (`{ name, description, ... }`).
pub(super) async fn handle_agents_create(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
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
pub(super) async fn handle_agents_delete(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
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

// ── Cron ────────────────────────────────────────────────────────────────

/// Access the cron scheduler, if it is running.
async fn cron_scheduler(
    state: &Arc<GatewayState>,
) -> Result<std::sync::Arc<tokio::sync::Mutex<crate::cron::cron::CronScheduler>>, WsResponse> {
    match state.scheduler.cron_scheduler.read().await.clone() {
        Some(s) => Ok(s),
        None => {
            Err(WsResponse::err(&"req".to_string(), "UNAVAILABLE", "Cron scheduler not running"))
        }
    }
}

/// `cron.get` — one job (`{ id }`).
pub(super) async fn handle_cron_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let sched = match cron_scheduler(state).await {
        Ok(s) => s,
        Err(res) => return res,
    };
    let guard = sched.lock().await;
    match guard.get_job(&id).await {
        Some(job) => WsResponse::ok(&req.id, serde_json::to_value(&job).unwrap_or_default()),
        None => WsResponse::err(&req.id, "NOT_FOUND", "cron job not found"),
    }
}

/// `cron.enable` / `cron.disable` — `{ id, enabled }`.
pub(super) async fn handle_cron_set_enabled(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    enabled: bool,
) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let sched = match cron_scheduler(state).await {
        Ok(s) => s,
        Err(res) => return res,
    };
    let guard = sched.lock().await;
    match guard.set_job_enabled(&id, enabled).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "success": true, "id": id, "enabled": enabled }),
        ),
        Err(e) => WsResponse::err(&req.id, "NOT_FOUND", &e.to_string()),
    }
}

/// `cron.run` — trigger a job immediately (`{ id }`).
pub(super) async fn handle_cron_run(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let sched = match cron_scheduler(state).await {
        Ok(s) => s,
        Err(res) => return res,
    };
    let guard = sched.lock().await;
    match guard.trigger_job(&id).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "success": true, "id": id, "triggered": true }),
        ),
        Err(e) => WsResponse::err(&req.id, "NOT_FOUND", &e.to_string()),
    }
}

/// `cron.logs` — job state / last-run info (`{ id }`).
pub(super) async fn handle_cron_logs(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    handle_cron_get(req, state).await
}

/// `cron.add` — add a job (`{ name, schedule, command }`).
pub(super) async fn handle_cron_add(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        schedule: String,
        command: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    use std::str::FromStr;
    let schedule = match cron::Schedule::from_str(&p.schedule) {
        Ok(_) => crate::cron::cron::Schedule::Cron {
            expression: p.schedule.clone(),
            timezone: None,
            stagger_ms: None,
        },
        Err(e) => {
            return WsResponse::err(
                &req.id,
                "INVALID_PARAMS",
                &format!("Invalid cron expression: {}", e),
            );
        }
    };
    let job_id = uuid::Uuid::new_v4().to_string();
    let job = crate::cron::cron::CronJob::new(
        job_id.clone(),
        p.name.clone(),
        schedule,
        crate::cron::cron::ExecutionTarget::shell(p.command),
    );
    let sched = match cron_scheduler(state).await {
        Ok(s) => s,
        Err(res) => return res,
    };
    let guard = sched.lock().await;
    match guard.add_job(job).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "success": true, "id": job_id, "name": p.name }),
        ),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", &format!("Failed to add job: {}", e)),
    }
}

/// `cron.remove` — remove a job (`{ id }`).
pub(super) async fn handle_cron_remove(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let sched = match cron_scheduler(state).await {
        Ok(s) => s,
        Err(res) => return res,
    };
    let guard = sched.lock().await;
    match guard.remove_job(&id).await {
        Ok(()) => WsResponse::ok(&req.id, serde_json::json!({ "success": true, "id": id })),
        Err(e) => WsResponse::err(&req.id, "NOT_FOUND", &e.to_string()),
    }
}

// ── Skills ──────────────────────────────────────────────────────────────

/// `skills.get` — one skill (`{ name }`).
pub(super) async fn handle_skills_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let name = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["name"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let sm = state.tools.skills_manager.read().await;
    match sm.get_skill(&name).await {
        Some(skill) => WsResponse::ok(&req.id, serde_json::to_value(&skill).unwrap_or_default()),
        None => WsResponse::err(&req.id, "NOT_FOUND", "skill not found"),
    }
}

/// `skills.enable` / `skills.disable` — `{ id, enabled }`.
pub(super) async fn handle_skills_set_enabled(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    enabled: bool,
) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let mut sm = state.tools.skills_manager.write().await;
    match sm.set_skill_enabled(&id, enabled).await {
        Ok(()) => WsResponse::ok(&req.id, serde_json::json!({ "success": true, "id": id })),
        Err(e) => WsResponse::err(&req.id, "NOT_FOUND", &e.to_string()),
    }
}

/// `skills.uninstall` — remove a skill (`{ name }`).
pub(super) async fn handle_skills_uninstall(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let name = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["name"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let sm = state.tools.skills_manager.read().await;
    match sm.uninstall_skill(&name).await {
        Ok(_) => WsResponse::ok(&req.id, serde_json::json!({ "success": true })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", &e.to_string()),
    }
}

/// `skills.run` — activate a skill (`{ id }`).
pub(super) async fn handle_skills_run(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let sm = state.tools.skills_manager.read().await;
    match sm.activate_skill(&id).await {
        Ok(_) => WsResponse::ok(&req.id, serde_json::json!({ "success": true, "id": id })),
        Err(e) => WsResponse::err(&req.id, "NOT_FOUND", &e.to_string()),
    }
}

/// `plugins.reload_all` — re-initialize the plugin manager (reload all).
pub(super) async fn handle_plugins_reload_all(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    match state.infra.plugin_manager.initialize().await {
        Ok(_) => WsResponse::ok(&req.id, serde_json::json!({ "success": true })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", &e.to_string()),
    }
}

/// `plugins.sign` — sign a plugin manifest with an ed25519 key.
pub(super) async fn handle_plugins_sign(req: &WsRequest, _state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        secret_key: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let signing_key = if p.secret_key.is_empty() {
        match crate::secrets::route_store("plugin")
            .get(&crate::secrets::SecretId::new("plugin", &p.name, "secret_key"))
            .await
        {
            Ok(Some(key)) => key,
            _ => {
                return WsResponse::err(
                    &req.id,
                    "BAD_REQUEST",
                    &format!(
                        "No signing key for plugin '{}'; submit secret_key in the request body",
                        p.name
                    ),
                );
            }
        }
    } else {
        if let Err(e) = crate::secrets::route_store("plugin")
            .set(
                &crate::secrets::SecretId::new("plugin", &p.name, "secret_key"),
                &p.secret_key,
                crate::secrets::SecretOrigin::UserEntered,
            )
            .await
        {
            eprintln!("Failed to store plugin secret_key for '{}' ({:?})", p.name, e);
        }
        p.secret_key.clone()
    };

    let manifest_path = crate::dirs::config_dir()
        .join("plugins")
        .join(&p.name)
        .join("plugin.json");
    let manifest_text = match tokio::fs::read_to_string(&manifest_path).await {
        Ok(t) => t,
        Err(e) => {
            return WsResponse::err(
                &req.id,
                "NOT_FOUND",
                &format!("Plugin '{}' not found at {:?}: {}", p.name, manifest_path, e),
            );
        }
    };
    let mut manifest: crate::plugins::manifest::PluginManifest =
        match serde_json::from_str(&manifest_text) {
            Ok(m) => m,
            Err(e) => {
                return WsResponse::err(
                    &req.id,
                    "BAD_REQUEST",
                    &format!("Invalid manifest: {}", e),
                );
            }
        };
    if let Err(e) = crate::plugins::verification::sign_manifest(&mut manifest, &signing_key) {
        return WsResponse::err(&req.id, "INTERNAL", &format!("Failed to sign manifest: {}", e));
    }
    if let Err(e) = tokio::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    )
    .await
    {
        return WsResponse::err(
            &req.id,
            "INTERNAL",
            &format!("Failed to write signed manifest: {}", e),
        );
    }
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "success": true,
            "message": format!("Plugin '{}' signed successfully", p.name),
            "signer_public_key": manifest.signer_public_key,
        }),
    )
}

// ── Providers ───────────────────────────────────────────────────────────

/// `providers.health` — one provider's health (`{ id }`).
pub(super) async fn handle_providers_health(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.infra.model_router.get_provider_health(&id).await {
        Some(health) => WsResponse::ok(&req.id, serde_json::json!({ "id": id, "health": health })),
        None => WsResponse::err(&req.id, "NOT_FOUND", "provider not found"),
    }
}

/// `providers.check` — force a health check (`{ id }`).
pub(super) async fn handle_providers_check(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.infra.model_router.check_provider_health(&id).await {
        Ok(r) => WsResponse::ok(&req.id, serde_json::json!({ "id": id, "healthy": r })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", &e.to_string()),
    }
}

/// `providers.switch` — set the default model (`{ model }`).
pub(super) async fn handle_providers_switch(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let model = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["model"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.infra.model_router.switch_default_model(&model).await {
        Ok(()) => WsResponse::ok(&req.id, serde_json::json!({ "success": true, "model": model })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", &e.to_string()),
    }
}

/// `models.default` — the current default model.
pub(super) async fn handle_models_default(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let default = state.infra.model_router.get_default_model().await;
    WsResponse::ok(&req.id, serde_json::json!({ "default_model": default }))
}

/// `providers.fallback` — the fallback chain for a model (`{ model_id }`).
pub(super) async fn handle_providers_fallback(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let model_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["model_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let chain = state.infra.model_router.get_fallback_chain(&model_id).await;
    WsResponse::ok(&req.id, serde_json::json!({ "model_id": model_id, "fallback_chain": chain }))
}

// ── Status ──────────────────────────────────────────────────────────────

/// `status.get` — engine status (agents, channels, version, cloud block).
pub(super) async fn handle_status_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agents = state.agents.agents.read().await;
    let channels = state.channels.channels.read().await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "agents": {
                "total": agents.len(),
                "busy": agents.values().filter(|a| a.busy.load(std::sync::atomic::Ordering::Acquire)).count(),
            },
            "channels": channels.len(),
            "version": crate::VERSION,
            "cloud": cloud_status_json(state).await,
        }),
    )
}

// ── MCP ─────────────────────────────────────────────────────────────────

/// `mcp.tools` — list a server's tools (`{ server_id }`).
pub(super) async fn handle_mcp_tools(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let server_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["server_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            let tools = client.get_tools().to_vec();
            WsResponse::ok(&req.id, serde_json::json!({ "tools": tools }))
        }
        None => WsResponse::err(&req.id, "NOT_FOUND", "MCP server not connected"),
    }
}

/// `mcp.call_tool` — invoke a tool on a connected server (`{ server_id, tool, args }`).
pub(super) async fn handle_mcp_call_tool(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let server_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["server_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let tool_name = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["tool"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let args = req.params.clone().and_then(|p| p["args"].clone().into());
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client
                .call_tool(&tool_name, args.unwrap_or(serde_json::json!({})))
                .await
            {
                Ok(result) => WsResponse::ok(&req.id, serde_json::json!({ "result": result })),
                Err(e) => WsResponse::err(&req.id, "INTERNAL", &e.to_string()),
            }
        }
        None => {
            WsResponse::err(&req.id, "NOT_FOUND", &format!("MCP server '{}' not found", server_id))
        }
    }
}

/// `mcp.resources` — list a server's resources (`{ server_id }`).
pub(super) async fn handle_mcp_resources(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let server_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["server_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.tools.mcp_manager.get_client(&server_id).await {
        Some(client_arc) => {
            let client = client_arc.read().await;
            match client.list_resources().await {
                Ok(resources) => {
                    WsResponse::ok(&req.id, serde_json::json!({ "resources": resources }))
                }
                Err(e) => WsResponse::err(&req.id, "INTERNAL", &e.to_string()),
            }
        }
        None => WsResponse::err(&req.id, "NOT_FOUND", "MCP server not connected"),
    }
}

/// `mcp.auth_status` — whether a server has a stored OAuth token (`{ server_id }`).
pub(super) async fn handle_mcp_auth_status(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let server_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["server_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let authorized = state.tools.mcp_manager.has_stored_token(&server_id).await;
    WsResponse::ok(&req.id, serde_json::json!({ "server_id": server_id, "authorized": authorized }))
}

// ── Device pairing ──────────────────────────────────────────────────────

/// `device.pairing.pending` — pending pairing requests.
pub(super) async fn handle_device_pairing_pending(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let pending = state.auth.device_pairing_store.list_pending().await;
    WsResponse::ok(&req.id, serde_json::json!({ "pending": pending }))
}

/// `device.pairing.authorized` — authorized devices.
pub(super) async fn handle_device_pairing_authorized(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let devices = state.auth.device_pairing_store.list_authorized().await;
    WsResponse::ok(&req.id, serde_json::json!({ "devices": devices }))
}

/// `device.pairing.approve` — approve a pairing request (`{ code }`).
pub(super) async fn handle_device_pairing_approve(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let code = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["code"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state
        .auth
        .device_pairing_store
        .approve(&code, Some("admin"))
        .await
    {
        Some(_) => WsResponse::ok(&req.id, serde_json::json!({ "status": "approved" })),
        None => WsResponse::err(&req.id, "NOT_FOUND", "pairing request not found or expired"),
    }
}

/// `device.pairing.reject` — reject a pairing request (`{ code }`).
pub(super) async fn handle_device_pairing_reject(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let code = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["code"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.auth.device_pairing_store.reject(&code).await {
        Some(_) => WsResponse::ok(&req.id, serde_json::json!({ "status": "rejected" })),
        None => WsResponse::err(&req.id, "NOT_FOUND", "pairing request not found"),
    }
}

/// `device.pairing.revoke` — revoke an authorized device (`{ device_id }`).
pub(super) async fn handle_device_pairing_revoke(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let device_id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["device_id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.auth.device_pairing_store.revoke(&device_id).await {
        true => WsResponse::ok(&req.id, serde_json::json!({ "status": "revoked" })),
        false => WsResponse::err(&req.id, "NOT_FOUND", "device not found"),
    }
}

/// `device.pairing.qr` — the pairing QR SVG for a pending code
/// (`{ code }`, returns `{ svg }`). SVG is text/XML so it fits in a WS
/// payload; formerly `GET /api/v1/device/pairing/qr/:code`.
pub(super) async fn handle_device_pairing_qr(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let code = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["code"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let pending = state.auth.device_pairing_store.list_pending().await;
    if !pending.iter().any(|r| r.code == code) {
        return WsResponse::err(&req.id, "NOT_FOUND", "pairing code not found or expired");
    }
    let uri = crate::security::device_pairing::DevicePairingStore::pairing_uri(&code);
    match crate::security::device_pairing::DevicePairingStore::generate_qr_svg(&uri) {
        Ok(svg) => WsResponse::ok(&req.id, serde_json::json!({ "code": code, "svg": svg })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", &e),
    }
}

/// `device.pairing.setup` — decode a base64url setup token and return the
/// pending request details (`{ setup_code }`). Formerly
/// `GET /api/v1/device/pairing/setup/:setup_code`.
pub(super) async fn handle_device_pairing_setup(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    use std::time::SystemTime;
    let setup_code = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["setup_code"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    let code =
        match crate::security::device_pairing::DevicePairingStore::decode_setup_code(&setup_code) {
            Some(code) => code,
            None => return WsResponse::err(&req.id, "INVALID_PARAMS", "invalid setup code"),
        };
    let pending = state.auth.device_pairing_store.list_pending().await;
    match pending.into_iter().find(|r| r.code == code) {
        Some(req_) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "code": req_.code,
                "device_id": req_.device_id,
                "display_name": req_.display_name,
                "expires_at": req_.expires_at.duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            }),
        ),
        None => WsResponse::err(&req.id, "NOT_FOUND", "pairing code not found or expired"),
    }
}

// ── Cloud ───────────────────────────────────────────────────────────────

/// Build the cloud status block (mirrors `status_handler`'s cloud JSON).
async fn cloud_status_json(state: &GatewayState) -> serde_json::Value {
    #[cfg(feature = "cloud")]
    {
        let cfg = { state.config.read().await.cloud.clone() };
        if !cfg.enabled {
            return serde_json::json!({ "enabled": false, "logged_in": false, "user": null });
        }
        let logged_in = crate::cloud::session::logged_in().await;
        let mut user = None;
        if logged_in {
            if let Some(token) = crate::cloud::session::get_token().await {
                if let Ok(Some(u)) = crate::cloud::client::CloudClient::new(&cfg, token)
                    .me()
                    .await
                {
                    user = Some(u);
                }
            }
        }
        serde_json::json!({ "enabled": true, "logged_in": logged_in, "user": user })
    }
    #[cfg(not(feature = "cloud"))]
    {
        let _ = state;
        serde_json::json!(null)
    }
}

/// `cloud.status` — `{ enabled, logged_in, user }` (or null without the cloud
/// feature).
pub(super) async fn handle_cloud_status(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    WsResponse::ok(&req.id, cloud_status_json(state).await)
}

/// `cloud.subscription` — plan + credit balance.
pub(super) async fn handle_cloud_subscription(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[cfg(feature = "cloud")]
    {
        let cfg = { state.config.read().await.cloud.clone() };
        if !cfg.enabled {
            return cloud_unavailable(req);
        }
        let Some(token) = crate::cloud::session::get_token().await else {
            return cloud_unavailable(req);
        };
        match crate::cloud::client::CloudClient::new(&cfg, token)
            .subscription()
            .await
        {
            Ok(v) => WsResponse::ok(&req.id, v),
            Err(e) => WsResponse::err(&req.id, "BAD_GATEWAY", e.to_string()),
        }
    }
    #[cfg(not(feature = "cloud"))]
    {
        let _ = state;
        cloud_unavailable(req)
    }
}

/// `cloud.usage` — `{ days }` credit usage for the period.
pub(super) async fn handle_cloud_usage(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[cfg(feature = "cloud")]
    #[derive(Deserialize)]
    struct UsageParams {
        #[serde(default = "default_usage_days")]
        days: u32,
    }
    #[cfg(feature = "cloud")]
    fn default_usage_days() -> u32 {
        30
    }
    #[cfg(feature = "cloud")]
    {
        let cfg = { state.config.read().await.cloud.clone() };
        if !cfg.enabled {
            return cloud_unavailable(req);
        }
        let Some(token) = crate::cloud::session::get_token().await else {
            return cloud_unavailable(req);
        };
        let days = parse_params::<UsageParams>(req)
            .map(|p| p.days)
            .unwrap_or(30);
        match crate::cloud::client::CloudClient::new(&cfg, token)
            .usage(days)
            .await
        {
            Ok(v) => WsResponse::ok(&req.id, v),
            Err(e) => WsResponse::err(&req.id, "BAD_GATEWAY", e.to_string()),
        }
    }
    #[cfg(not(feature = "cloud"))]
    {
        let _ = (state, req);
        cloud_unavailable(req)
    }
}

/// `cloud.token` — `{ token }` persist a cloud session token (OAuth result).
pub(super) async fn handle_cloud_token(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[cfg(feature = "cloud")]
    #[derive(Deserialize)]
    struct TokenParams {
        token: String,
    }
    #[cfg(feature = "cloud")]
    {
        let cfg = { state.config.read().await.cloud.clone() };
        if !cfg.enabled {
            return cloud_unavailable(req);
        }
        let params: TokenParams = match parse_params(req) {
            Ok(p) => p,
            Err(res) => return res,
        };
        if let Err(e) = crate::cloud::session::set_token(&params.token).await {
            return WsResponse::err(&req.id, "INTERNAL", e.to_string());
        }
        match crate::cloud::client::CloudClient::new(&cfg, params.token.clone())
            .me()
            .await
        {
            Ok(Some(user)) => {
                // Best-effort device registration (P2-9): a stable device
                // identity for future cloud sync. Never fails the login on
                // bind errors (mirrors the removed REST token handler).
                if let Err(e) = crate::cloud::device::bind(&cfg).await {
                    tracing::warn!("Cloud device bind failed: {e}");
                }
                WsResponse::ok(&req.id, serde_json::json!({ "ok": true, "user": user }))
            }
            Ok(None) => WsResponse::err(&req.id, "UNAUTHORIZED", "invalid cloud token"),
            Err(e) => WsResponse::err(&req.id, "BAD_GATEWAY", e.to_string()),
        }
    }
    #[cfg(not(feature = "cloud"))]
    {
        let _ = (state, req);
        cloud_unavailable(req)
    }
}

/// `cloud.logout` — forget the stored session token.
pub(super) async fn handle_cloud_logout(req: &WsRequest, _state: &Arc<GatewayState>) -> WsResponse {
    #[cfg(feature = "cloud")]
    {
        let _ = crate::cloud::session::clear_token().await;
    }
    WsResponse::ok(&req.id, serde_json::json!({ "ok": true }))
}

// ── Update ──────────────────────────────────────────────────────────────

/// `update.status` — current/latest release info.
pub(super) async fn handle_update_status(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    if let Some(cache) = state.update.status_cache.read().await.as_ref() {
        WsResponse::ok(&req.id, serde_json::to_value(&cache.info).unwrap_or_default())
    } else {
        WsResponse::ok(&req.id, serde_json::json!({ "enabled": false }))
    }
}

/// `update.progress` — current update phase/percent (polled).
pub(super) async fn handle_update_progress(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let progress = state.update.progress.read().await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "phase": progress.phase,
            "percent": progress.percent,
            "error": progress.error,
            "current": progress.current,
            "latest": progress.latest,
        }),
    )
}

/// `update.trigger` — start the self-update flow (same checks as
/// `POST /api/v1/update`). Rejected when embedded (desktop uses the Tauri
/// updater instead) or disabled in config.
pub(super) async fn handle_update_trigger(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    use crate::gateway::handlers::update::{run_update_task, set_progress};
    use crate::gateway::state::{UpdatePhase, UpdateProgress};

    if state.embedded {
        return WsResponse::err(
            &req.id,
            "CONFLICT",
            "This syscity instance is embedded in the desktop app; use the desktop updater instead.",
        );
    }
    if !state.config.read().await.update.enabled {
        return WsResponse::err(
            &req.id,
            "FORBIDDEN",
            "Online updates are disabled in the configuration.",
        );
    }
    {
        let progress = state.update.progress.read().await;
        let busy = matches!(
            progress.phase,
            UpdatePhase::Checking
                | UpdatePhase::Downloading
                | UpdatePhase::Verifying
                | UpdatePhase::Applying
                | UpdatePhase::Restarting
        );
        if busy {
            return WsResponse::err(&req.id, "CONFLICT", "An update is already in progress.");
        }
    }

    *state.update.progress.write().await = UpdateProgress::idle(crate::VERSION);
    set_progress(state, UpdatePhase::Checking, 5, None).await;

    let host = state.config.read().await.host.clone();
    let port = state.config.read().await.port;
    let task_state = state.clone();
    let shutdown_token = state.shutdown_token.clone();
    let handle = tokio::spawn(async move {
        run_update_task(task_state, shutdown_token, host, port).await;
    });
    state
        .task_registry
        .insert_join("update:apply", handle)
        .await;

    WsResponse::ok(&req.id, serde_json::json!({ "status": "started" }))
}

// ── Plugins ─────────────────────────────────────────────────────────────

/// `plugins.list` — installed plugins.
pub(super) async fn handle_plugins_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let plugins = state.infra.plugin_manager.list_plugins().await;
    let plugin_list: Vec<_> = plugins
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id(),
                "name": p.name(),
                "enabled": p.enabled,
                "capabilities": p.manifest.capabilities,
            })
        })
        .collect();
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "plugins": plugin_list, "count": plugin_list.len() }),
    )
}

/// `plugins.enable` / `plugins.disable` — toggle a plugin.
pub(super) async fn handle_plugins_set_enabled(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    enabled: bool,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state.infra.plugin_manager.set_enabled(&p.id, enabled).await {
        Ok(()) => WsResponse::ok(&req.id, serde_json::json!({ "success": true })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
    }
}

/// `plugins.install` — install a plugin by name.
pub(super) async fn handle_plugins_install(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        registry: Option<String>,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state
        .infra
        .plugin_manager
        .install_plugin(&p.name, p.registry.as_deref())
        .await
    {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "success": true, "message": format!("Plugin '{}' installed", p.name) }),
        ),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", format!("Failed to install plugin: {}", e)),
    }
}

/// `plugins.search` — search the plugin registry.
pub(super) async fn handle_plugins_search(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        q: String,
        registry: Option<String>,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state
        .infra
        .plugin_manager
        .search_registry(&p.q, p.registry.as_deref())
        .await
    {
        Ok(results) => WsResponse::ok(&req.id, serde_json::json!({ "results": results })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
    }
}

/// `plugins.unload` — unload a plugin (disable at runtime, keep on disk).
pub(super) async fn handle_plugins_unload(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state.infra.plugin_manager.unload_plugin(&p.id).await {
        Ok(_) => WsResponse::ok(&req.id, serde_json::json!({ "success": true })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
    }
}

/// `plugins.reload` — reload a plugin's manifest/runtime.
pub(super) async fn handle_plugins_reload(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state.infra.plugin_manager.reload_plugin(&p.id).await {
        Ok(_) => WsResponse::ok(&req.id, serde_json::json!({ "success": true })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
    }
}

/// `plugins.uninstall` — remove a plugin from disk.
pub(super) async fn handle_plugins_uninstall(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state.infra.plugin_manager.uninstall_plugin(&p.name).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "success": true, "message": format!("Plugin '{}' uninstalled", p.name) }),
        ),
        Err(e) => {
            WsResponse::err(&req.id, "INTERNAL", format!("Failed to uninstall plugin: {}", e))
        }
    }
}

// ── Providers ───────────────────────────────────────────────────────────

/// `providers.list` — configured model providers.
pub(super) async fn handle_providers_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let providers = state.infra.model_router.list_providers().await;
    WsResponse::ok(&req.id, serde_json::json!({ "providers": providers }))
}

/// `providers.enable` / `providers.disable` — toggle a provider.
pub(super) async fn handle_providers_set_enabled(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    enabled: bool,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let result = if enabled {
        state.infra.model_router.enable_provider(&p.id).await
    } else {
        state.infra.model_router.disable_provider(&p.id).await
    };
    match result {
        Ok(()) => WsResponse::ok(&req.id, serde_json::json!({ "success": true })),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
    }
}

/// `providers.usage` — provider usage snapshots with quota.
pub(super) async fn handle_providers_usage(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let snapshots = state.infra.model_router.all_snapshots_with_quota().await;
    WsResponse::ok(&req.id, serde_json::json!({ "usage": snapshots }))
}

// ── Approvals (human-in-the-loop tool approval) ─────────────────────────

/// `approvals.list` — pending tool-call approval requests.
pub(super) async fn handle_approvals_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let approvals = state
        .tools
        .approval_queue
        .list_pending(crate::tools::approval::ApprovalFilter::default())
        .await;
    WsResponse::ok(&req.id, serde_json::json!({ "approvals": approvals, "count": approvals.len() }))
}

/// `approvals.get` — a single pending approval (`{ id }`).
pub(super) async fn handle_approvals_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.tools.approval_queue.get(&id).await {
        Some(approval) => {
            WsResponse::ok(&req.id, serde_json::to_value(&approval).unwrap_or_default())
        }
        None => WsResponse::err(&req.id, "NOT_FOUND", &format!("Approval '{}' not found", id)),
    }
}

/// `approvals.approve` — approve a pending tool call (`{ id }`).
pub(super) async fn handle_approvals_approve(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    if state
        .tools
        .approval_queue
        .resolve(&id, crate::tools::approval::ApprovalDecision::Approve)
        .await
    {
        WsResponse::ok(&req.id, serde_json::json!({ "id": id, "status": "approved" }))
    } else {
        WsResponse::err(&req.id, "NOT_FOUND", &format!("Approval '{}' not found", id))
    }
}

/// `approvals.deny` — deny a pending tool call (`{ id, reason? }`).
pub(super) async fn handle_approvals_deny(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Deserialize)]
    struct Params {
        id: String,
        #[serde(default)]
        reason: Option<String>,
    }
    let p: Params = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let reason = p.reason.unwrap_or_else(|| "Denied by operator".to_string());
    if state
        .tools
        .approval_queue
        .resolve(&p.id, crate::tools::approval::ApprovalDecision::Deny { reason: reason.clone() })
        .await
    {
        WsResponse::ok(
            &req.id,
            serde_json::json!({ "id": p.id, "status": "denied", "reason": reason }),
        )
    } else {
        WsResponse::err(&req.id, "NOT_FOUND", &format!("Approval '{}' not found", p.id))
    }
}

// ── Memory search / add (vector memory admin) ───────────────────────────

/// `memory.search` — `{ query, limit?, collection?, threshold? }`.
pub(super) async fn handle_memory_search(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let body: crate::gateway::types::MemorySearchRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state.memory.vector.read().await.clone() {
        Some(vm) => match vm
            .search_collection(&body.query, body.limit, &body.collection, body.threshold)
            .await
        {
            Ok(results) => WsResponse::ok(
                &req.id,
                serde_json::json!({ "query": body.query, "results": results, "count": results.len() }),
            ),
            Err(e) => WsResponse::err(&req.id, "INTERNAL", &format!("Search failed: {}", e)),
        },
        None => WsResponse::err(&req.id, "UNAVAILABLE", "Vector memory service not enabled"),
    }
}

/// `memory.add` — `{ content, metadata?, collection? }`.
pub(super) async fn handle_memory_add(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let body: crate::gateway::types::MemoryAddRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    match state.memory.vector.read().await.clone() {
        Some(vm) => match vm
            .add_to_collection(&body.content, body.metadata, &body.collection)
            .await
        {
            Ok(doc_id) => WsResponse::ok(
                &req.id,
                serde_json::json!({ "document_id": doc_id, "status": "added" }),
            ),
            Err(e) => {
                WsResponse::err(&req.id, "INTERNAL", &format!("Failed to add document: {}", e))
            }
        },
        None => WsResponse::err(&req.id, "UNAVAILABLE", "Vector memory service not enabled"),
    }
}

/// `memory.collections` — list vector memory collections.
pub(super) async fn handle_memory_collections(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    match state.memory.vector.read().await.clone() {
        Some(vm) => {
            let collections = vm.list_collections().await;
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "collections": collections, "count": collections.len() }),
            )
        }
        None => WsResponse::err(&req.id, "UNAVAILABLE", "Vector memory service not enabled"),
    }
}

// ── Mention gate policy / allowlist / blocklist ─────────────────────────

/// `mention.policy` — current mention gate policy.
pub(super) async fn handle_mention_policy_get(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let policy = state.auth.mention_gate.policy().await;
    WsResponse::ok(&req.id, serde_json::json!({ "policy": policy.to_string() }))
}

/// `mention.policy.set` — `{ policy }`.
pub(super) async fn handle_mention_policy_set(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::SetMentionPolicyRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    state.auth.mention_gate.set_policy(body.policy).await;
    let policy = state.auth.mention_gate.policy().await;
    WsResponse::ok(&req.id, serde_json::json!({ "status": "ok", "policy": policy.to_string() }))
}

/// `mention.allowlist` — `{ channel? }` (default "*") list allowlist entries.
pub(super) async fn handle_mention_allowlist_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let channel = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["channel"].as_str().unwrap_or("*").to_string(),
        Err(res) => return res,
    };
    let entries = state.auth.mention_gate.list_allowlist(&channel).await;
    WsResponse::ok(&req.id, serde_json::json!({ "channel": channel, "allowlist": entries }))
}

/// `mention.allowlist.add` — `{ channel, pattern }`.
pub(super) async fn handle_mention_allowlist_add(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::AddMentionPatternRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    state
        .auth
        .mention_gate
        .add_allowlist(&body.channel, &body.pattern)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "status": "added", "channel": body.channel, "pattern": body.pattern }),
    )
}

/// `mention.allowlist.remove` — `{ channel, pattern }`.
pub(super) async fn handle_mention_allowlist_remove(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::AddMentionPatternRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let removed = state
        .auth
        .mention_gate
        .remove_allowlist(&body.channel, &body.pattern)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": if removed { "removed" } else { "not_found" },
            "channel": body.channel,
            "pattern": body.pattern,
        }),
    )
}

/// `mention.blocklist` — `{ channel? }` (default "*") list blocklist entries.
pub(super) async fn handle_mention_blocklist_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let channel = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["channel"].as_str().unwrap_or("*").to_string(),
        Err(res) => return res,
    };
    let entries = state.auth.mention_gate.list_blocklist(&channel).await;
    WsResponse::ok(&req.id, serde_json::json!({ "channel": channel, "blocklist": entries }))
}

/// `mention.blocklist.add` — `{ channel, pattern }`.
pub(super) async fn handle_mention_blocklist_add(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::AddMentionPatternRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    state
        .auth
        .mention_gate
        .add_blocklist(&body.channel, &body.pattern)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "status": "added", "channel": body.channel, "pattern": body.pattern }),
    )
}

/// `mention.blocklist.remove` — `{ channel, pattern }`.
pub(super) async fn handle_mention_blocklist_remove(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let body: crate::gateway::types::AddMentionPatternRequest = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };
    let removed = state
        .auth
        .mention_gate
        .remove_blocklist(&body.channel, &body.pattern)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": if removed { "removed" } else { "not_found" },
            "channel": body.channel,
            "pattern": body.pattern,
        }),
    )
}

// ── Auth profiles (provider API-key state) ──────────────────────────────

/// `auth_profiles.list` — all auth profiles across providers.
pub(super) async fn handle_auth_profiles_list(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let profiles = state.infra.model_router.list_auth_profiles().await;
    WsResponse::ok(&req.id, serde_json::json!({ "profiles": profiles, "count": profiles.len() }))
}

/// `auth_profiles.get` — auth profile status for a provider (`{ id }`).
pub(super) async fn handle_auth_profiles_get(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.infra.model_router.get_auth_profile_status(&id).await {
        Some(status) => WsResponse::ok(&req.id, serde_json::to_value(status).unwrap_or_default()),
        None => WsResponse::err(
            &req.id,
            "NOT_FOUND",
            &format!("No auth profile found for provider '{}'", id),
        ),
    }
}

/// `auth_profiles.rotate` — rotate a provider's API key (`{ id }`).
pub(super) async fn handle_auth_profiles_rotate(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let id = match parse_params::<serde_json::Value>(req) {
        Ok(v) => v["id"].as_str().unwrap_or("").to_string(),
        Err(res) => return res,
    };
    match state.infra.model_router.rotate_auth_key(&id).await {
        Ok(_new_key) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "success": true,
                "provider": id,
                "message": format!("Auth key rotated for provider '{}'", id),
            }),
        ),
        Err(e) => {
            WsResponse::err(&req.id, "BAD_REQUEST", &format!("Failed to rotate auth key: {}", e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;

    fn req(id: &str, method: &str, params: Option<serde_json::Value>) -> WsRequest {
        WsRequest {
            frame_type: "req".into(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }

    async fn state() -> Arc<GatewayState> {
        Arc::new(make_test_state(GatewayConfig::default()).await)
    }

    #[tokio::test]
    async fn system_reload_defaults_to_all() {
        let state = state().await;
        let resp =
            handle_system_reload(&req("r1", "system.reload", Some(serde_json::json!({}))), &state)
                .await;
        assert!(resp.ok, "reload failed: {:?}", resp.error);
        let payload = resp.payload.as_ref().unwrap();
        assert_eq!(payload["scope"], "all");
        assert_eq!(payload["success"], true);
    }

    #[tokio::test]
    async fn system_reload_scope_skills() {
        let state = state().await;
        let resp = handle_system_reload(
            &req("r1", "system.reload", Some(serde_json::json!({ "scope": "skills" }))),
            &state,
        )
        .await;
        assert!(resp.ok);
        assert!(resp.payload.as_ref().unwrap()["skills"].is_object());
    }

    #[tokio::test]
    async fn channels_list_empty() {
        let state = state().await;
        let resp = handle_channels_list(&req("r1", "channels.list", None), &state).await;
        assert!(resp.ok);
        assert_eq!(
            resp.payload.as_ref().unwrap()["channels"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn channels_enable_unknown_not_found() {
        let state = state().await;
        let resp = handle_channels_enable(
            &req("r1", "channels.enable", Some(serde_json::json!({ "name": "telegram" }))),
            &state,
        )
        .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "NOT_FOUND");
    }

    #[tokio::test]
    async fn channels_disable_missing_name_errors() {
        let state = state().await;
        let resp = handle_channels_disable(
            &req("r1", "channels.disable", Some(serde_json::json!({}))),
            &state,
        )
        .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_PARAMS");
    }

    #[tokio::test]
    async fn device_pairing_qr_unknown_not_found() {
        let state = state().await;
        let resp = handle_device_pairing_qr(
            &req("r1", "device.pairing.qr", Some(serde_json::json!({ "code": "NOPE" }))),
            &state,
        )
        .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "NOT_FOUND");
    }

    #[tokio::test]
    async fn device_pairing_qr_seeded_returns_svg() {
        let state = state().await;
        let code = match state
            .auth
            .device_pairing_store
            .request_access("dev-1", Some("Phone"), None)
            .await
        {
            crate::security::device_pairing::DeviceAccessResult::PairingRequired { code } => code,
            _ => panic!("expected a new pending request"),
        };
        let resp = handle_device_pairing_qr(
            &req("r1", "device.pairing.qr", Some(serde_json::json!({ "code": code }))),
            &state,
        )
        .await;
        assert!(resp.ok, "qr failed: {:?}", resp.error);
        let svg = resp.payload.as_ref().unwrap()["svg"].as_str().unwrap();
        assert!(svg.contains("<svg"));
    }

    #[tokio::test]
    async fn device_pairing_setup_roundtrip() {
        let state = state().await;
        let code = match state
            .auth
            .device_pairing_store
            .request_access("dev-2", Some("Tablet"), None)
            .await
        {
            crate::security::device_pairing::DeviceAccessResult::PairingRequired { code } => code,
            _ => panic!("expected a new pending request"),
        };
        let setup_code =
            crate::security::device_pairing::DevicePairingStore::encode_setup_code(&code);
        let resp = handle_device_pairing_setup(
            &req(
                "r1",
                "device.pairing.setup",
                Some(serde_json::json!({ "setup_code": setup_code })),
            ),
            &state,
        )
        .await;
        assert!(resp.ok, "setup failed: {:?}", resp.error);
        let payload = resp.payload.as_ref().unwrap();
        assert_eq!(payload["device_id"], "dev-2");
        assert_eq!(payload["display_name"], "Tablet");
    }

    #[tokio::test]
    async fn device_pairing_setup_invalid_code_errors() {
        let state = state().await;
        let resp = handle_device_pairing_setup(
            &req(
                "r1",
                "device.pairing.setup",
                Some(serde_json::json!({ "setup_code": "!!not-base64!!" })),
            ),
            &state,
        )
        .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "INVALID_PARAMS");
    }

    #[tokio::test]
    async fn approvals_list_empty() {
        let state = state().await;
        let resp = handle_approvals_list(&req("r1", "approvals.list", None), &state).await;
        assert!(resp.ok);
        assert_eq!(resp.payload.as_ref().unwrap()["count"], 0);
    }

    #[tokio::test]
    async fn approvals_approve_unknown_not_found() {
        let state = state().await;
        let resp = handle_approvals_approve(
            &req("r1", "approvals.approve", Some(serde_json::json!({ "id": "missing" }))),
            &state,
        )
        .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "NOT_FOUND");
    }

    #[tokio::test]
    async fn approvals_submit_then_deny_with_reason() {
        let state = state().await;
        // Submit a pending approval with a live response channel.
        let (tx, _rx) = tokio::sync::oneshot::channel();
        let pa = crate::tools::approval::PendingApproval::new(
            "app-1",
            "bash",
            serde_json::json!({ "command": "ls" }),
            "alice",
        )
        .with_risk_level(crate::tools::approval::RiskLevel::High)
        .with_approval_level(crate::tools::approval::ApprovalLevel::Ask)
        .with_message("Run bash")
        .with_response_tx(tx);
        state.tools.approval_queue.submit(pa).await;

        let resp = handle_approvals_list(&req("r1", "approvals.list", None), &state).await;
        assert!(resp.ok);
        assert_eq!(resp.payload.as_ref().unwrap()["count"], 1);

        let resp = handle_approvals_deny(
            &req(
                "r1",
                "approvals.deny",
                Some(serde_json::json!({ "id": "app-1", "reason": "Not authorized" })),
            ),
            &state,
        )
        .await;
        assert!(resp.ok, "deny failed: {:?}", resp.error);
        let payload = resp.payload.as_ref().unwrap();
        assert_eq!(payload["status"], "denied");
        assert_eq!(payload["reason"], "Not authorized");
    }

    #[tokio::test]
    async fn memory_search_unavailable_without_vector() {
        let state = state().await;
        let resp = handle_memory_search(
            &req("r1", "memory.search", Some(serde_json::json!({ "query": "foo" }))),
            &state,
        )
        .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "UNAVAILABLE");
    }

    #[tokio::test]
    async fn memory_collections_unavailable_without_vector() {
        let state = state().await;
        let resp = handle_memory_collections(&req("r1", "memory.collections", None), &state).await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "UNAVAILABLE");
    }

    #[tokio::test]
    async fn mention_policy_get_and_set_roundtrip() {
        let state = state().await;
        let resp = handle_mention_policy_get(&req("r1", "mention.policy", None), &state).await;
        assert!(resp.ok);
        assert!(resp.payload.as_ref().unwrap()["policy"].is_string());

        let resp = handle_mention_policy_set(
            &req("r1", "mention.policy.set", Some(serde_json::json!({ "policy": "block" }))),
            &state,
        )
        .await;
        assert!(resp.ok, "set failed: {:?}", resp.error);
        assert_eq!(resp.payload.as_ref().unwrap()["policy"], "block");
    }

    #[tokio::test]
    async fn mention_allowlist_add_and_list() {
        let state = state().await;
        let resp = handle_mention_allowlist_add(
            &req(
                "r1",
                "mention.allowlist.add",
                Some(serde_json::json!({ "channel": "telegram", "pattern": "@boss" })),
            ),
            &state,
        )
        .await;
        assert!(resp.ok);
        let resp = handle_mention_allowlist_list(
            &req("r1", "mention.allowlist", Some(serde_json::json!({ "channel": "telegram" }))),
            &state,
        )
        .await;
        assert!(resp.ok);
        let entries = resp.payload.as_ref().unwrap()["allowlist"]
            .as_array()
            .unwrap();
        assert!(entries.iter().any(|e| e == "@boss"));
    }

    #[tokio::test]
    async fn auth_profiles_list_empty_and_get_unknown() {
        let state = state().await;
        let resp = handle_auth_profiles_list(&req("r1", "auth_profiles.list", None), &state).await;
        assert!(resp.ok);
        assert_eq!(resp.payload.as_ref().unwrap()["count"], 0);

        let resp = handle_auth_profiles_get(
            &req("r1", "auth_profiles.get", Some(serde_json::json!({ "id": "openai" }))),
            &state,
        )
        .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "NOT_FOUND");
    }

    #[tokio::test]
    async fn auth_profiles_rotate_unknown_errors() {
        let state = state().await;
        let resp = handle_auth_profiles_rotate(
            &req("r1", "auth_profiles.rotate", Some(serde_json::json!({ "id": "openai" }))),
            &state,
        )
        .await;
        assert!(!resp.ok);
        assert_eq!(resp.error.as_ref().unwrap().code, "BAD_REQUEST");
    }
}
