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
