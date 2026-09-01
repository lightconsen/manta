//! WebSocket RPC handlers for admin-style operations that previously had only
//! REST equivalents: plugins, providers, updates, cloud, onboarding, catalog
//! and config reload. These let the built-in UI drive everything over WS
//! (single transport, no CORS) while the REST surface stays for external
//! tools / CLI / OpenAI compatibility.

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

// ── Connectors catalog (marketplace) ────────────────────────────────────

/// `connectors.catalog` — the marketplace catalog document.
pub(super) async fn handle_connectors_catalog(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let manager = &state.tools.connector_manager;
    match manager.cached_catalog().await {
        Ok(Some(doc)) => WsResponse::ok(&req.id, doc),
        Ok(None) => WsResponse::ok(&req.id, serde_json::json!(null)),
        Err(e) => WsResponse::err(&req.id, "INTERNAL", e.to_string()),
    }
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
        .max_by_key(|e| crate::gateway::handlers::connectors::catalog_version_key(&e.version))
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

// ── Status ──────────────────────────────────────────────────────────────

/// `status.get` — engine status (agents, channels, version, cloud block).
pub(super) async fn handle_status_get(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let agents = state.agents.agents.read().await;
    let channels = state.channels.channels.read().await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "agents": { "total": agents.len(), "busy": agents.values().filter(|a| a.busy.load(std::sync::atomic::Ordering::Acquire)).count() },
            "channels": channels.len(),
            "version": crate::VERSION,
        }),
    )
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
        match crate::cloud::client::CloudClient::new(&cfg, params.token)
            .me()
            .await
        {
            Ok(Some(user)) => {
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
