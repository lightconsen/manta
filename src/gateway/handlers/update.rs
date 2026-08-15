//! Online self-update endpoints.
//!
//! `GET  /api/v1/update/status`   — latest release info (cached, TTL 6h)
//! `POST /api/v1/update`          — trigger a background update run
//! `GET  /api/v1/update/progress` — phase/percent of the running update
//!
//! The background run downloads and verifies the new binary, replaces the
//! running executable, spawns a detached `syscity restart --pid <self>`
//! helper, and then cancels the gateway shutdown token so the daemon exits and
//! the helper starts the new binary. The web view polls `/progress` meanwhile.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::gateway::state::{UpdatePhase, UpdateProgress, UpdateStatusCache};
use crate::gateway::GatewayState;

/// How long a checked release status is considered fresh before re-checking
/// GitHub. Bounds how often the banner endpoint hits the GitHub API.
const STATUS_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// GET /api/v1/update/status
///
/// Returns the current installed version and, if known, the latest published
/// version. Checks GitHub at most once per [`STATUS_TTL`]; pass `?refresh=1`
/// to bypass the cache for an explicit "check for updates".
pub async fn update_status_handler(
    State(state): State<Arc<GatewayState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    state.update.checks_total.fetch_add(1, Ordering::Relaxed);

    let enabled = state.config.read().await.update.enabled;
    let embedded = state.embedded;

    if !enabled {
        return Json(serde_json::json!({
            "enabled": false,
            "current": crate::VERSION,
            "update_available": false,
            "embedded": embedded,
        }));
    }

    let force = params.get("refresh").map(|v| v == "1").unwrap_or(false);
    let info = fetch_or_cached(&state, force).await;
    Json(serde_json::json!({
        "enabled": true,
        "current": info.current,
        "latest": info.latest,
        "update_available": info.update_available,
        "embedded": embedded,
    }))
}

/// POST /api/v1/update
///
/// Starts a background update run and returns `202 Accepted`. Refuses to run
/// when the instance is embedded (desktop) or an update is already in flight.
pub async fn trigger_update_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    if state.embedded {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "embedded",
                "message": "This syscity instance is embedded in the desktop app; \
                            use the desktop updater instead.",
            })),
        )
            .into_response();
    }

    if !state.config.read().await.update.enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "disabled",
                "message": "Online updates are disabled in the configuration.",
            })),
        )
            .into_response();
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
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "busy",
                    "message": "An update is already in progress.",
                })),
            )
                .into_response();
        }
    }

    *state.update.progress.write().await = UpdateProgress::idle(crate::VERSION);
    set_progress(&state, UpdatePhase::Checking, 5, None).await;

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

    (StatusCode::ACCEPTED, Json(serde_json::json!({ "status": "started" }))).into_response()
}

/// GET /api/v1/update/progress
pub async fn update_progress_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let progress = state.update.progress.read().await;
    Json(serde_json::json!({
        "phase": progress.phase,
        "percent": progress.percent,
        "error": progress.error,
        "current": progress.current,
        "latest": progress.latest,
    }))
}

/// Return the latest release info, using the in-memory cache when it is fresh.
async fn fetch_or_cached(state: &GatewayState, force: bool) -> crate::update::UpdateInfo {
    if !force {
        if let Some(cache) = state.update.status_cache.read().await.as_ref() {
            if cache.checked_at.elapsed() < STATUS_TTL {
                return cache.info.clone();
            }
        }
    }

    let client = reqwest::Client::new();
    match crate::update::github::check_latest(&client, crate::VERSION).await {
        Ok(info) => {
            let mut cache = state.update.status_cache.write().await;
            *cache = Some(UpdateStatusCache {
                info: info.clone(),
                checked_at: Instant::now(),
            });
            info
        }
        Err(e) => {
            warn!("Update status check failed: {}", e);
            // Fall back to the last cached result, else "up to date".
            state
                .update
                .status_cache
                .read()
                .await
                .as_ref()
                .map(|c| c.info.clone())
                .unwrap_or_else(|| crate::update::UpdateInfo::up_to_date(crate::VERSION))
        }
    }
}

/// The background update run: check → download → verify → apply → restart.
async fn run_update_task(
    state: Arc<GatewayState>,
    shutdown_token: CancellationToken,
    host: String,
    port: u16,
) {
    let client = reqwest::Client::new();

    let info = match crate::update::github::check_latest(&client, crate::VERSION).await {
        Ok(info) => info,
        Err(e) => {
            state.update.failures_total.fetch_add(1, Ordering::Relaxed);
            warn!("Update check failed: {}", e);
            fail(&state, e.to_string()).await;
            return;
        }
    };

    if !info.update_available {
        set_progress(&state, UpdatePhase::Idle, 100, None).await;
        return;
    }
    state.update.progress.write().await.latest = Some(info.latest.clone());

    let target = match crate::update::platform::asset_target() {
        Some(target) => target,
        None => {
            state.update.failures_total.fetch_add(1, Ordering::Relaxed);
            warn!("Update unsupported on {}/{}", std::env::consts::OS, std::env::consts::ARCH);
            fail(
                &state,
                format!(
                    "no release artifacts for {}/{}",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            )
            .await;
            return;
        }
    };

    set_progress(&state, UpdatePhase::Downloading, 20, None).await;
    let checksum = match crate::update::github::fetch_checksum(&client, &info.latest, target).await
    {
        Ok(checksum) => checksum,
        Err(e) => {
            state.update.failures_total.fetch_add(1, Ordering::Relaxed);
            warn!("Failed to fetch checksum: {}", e);
            fail(&state, e.to_string()).await;
            return;
        }
    };

    let url = crate::update::github::asset_download_url(&info.latest, target);
    let pkg = match crate::update::download::download_and_verify(&client, &url, &checksum).await {
        Ok(pkg) => pkg,
        Err(e) => {
            state.update.failures_total.fetch_add(1, Ordering::Relaxed);
            warn!("Update download failed: {}", e);
            fail(&state, e.to_string()).await;
            return;
        }
    };
    set_progress(&state, UpdatePhase::Verifying, 70, None).await;

    if let Err(e) = crate::update::apply::apply_binary(pkg.path()) {
        state.update.failures_total.fetch_add(1, Ordering::Relaxed);
        warn!("Update apply failed: {}", e);
        fail(&state, e.to_string()).await;
        return;
    }
    set_progress(&state, UpdatePhase::Applying, 90, None).await;

    // Spawn a detached helper that restarts the daemon once this process has
    // exited, then shut the gateway down so the new binary can bind the port.
    if let Err(e) =
        crate::daemon::DaemonManager::spawn_restart_helper(&host, port, std::process::id())
    {
        state.update.failures_total.fetch_add(1, Ordering::Relaxed);
        warn!("Failed to spawn restart helper: {}", e);
        fail(&state, e.to_string()).await;
        return;
    }
    set_progress(&state, UpdatePhase::Restarting, 95, None).await;
    shutdown_token.cancel();
}

/// Mark the current update run as failed with `message`.
async fn fail(state: &GatewayState, message: String) {
    let mut progress = state.update.progress.write().await;
    progress.phase = UpdatePhase::Error;
    progress.percent = 0;
    progress.error = Some(message);
}

/// Update the shared progress record.
async fn set_progress(
    state: &GatewayState,
    phase: UpdatePhase,
    percent: u8,
    error: Option<String>,
) {
    let mut progress = state.update.progress.write().await;
    progress.phase = phase;
    progress.percent = percent;
    progress.error = error;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;

    #[tokio::test]
    async fn progress_serializes_phase_snake_case() {
        let progress = UpdateProgress {
            phase: UpdatePhase::Downloading,
            percent: 20,
            error: None,
            current: "0.1.2".into(),
            latest: Some("0.2.0".into()),
        };
        let json = serde_json::to_value(progress).unwrap();
        assert_eq!(json["phase"], "downloading");
        assert_eq!(json["percent"], 20);
    }

    #[tokio::test]
    async fn status_handler_reports_up_to_date_without_network() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let resp = update_status_handler(State(state), Query(Default::default())).await;
        let body = resp.into_response();
        assert_eq!(body.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn disabled_config_rejects_trigger() {
        let mut config = GatewayConfig::default();
        config.update.enabled = false;
        let state = Arc::new(make_test_state(config).await);
        let resp = trigger_update_handler(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn embedded_instance_rejects_trigger() {
        let mut state = Arc::new(make_test_state(GatewayConfig::default()).await);
        Arc::get_mut(&mut state).unwrap().embedded = true;
        let resp = trigger_update_handler(State(state)).await.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}
