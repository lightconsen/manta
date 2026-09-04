//! Online self-update support.
//!
//! The UI drives updates over WebSocket (`update.status` / `update.trigger` /
//! `update.progress` in `ws/admin_ws.rs`); the REST update endpoints were
//! removed. The background run downloads and verifies the new binary,
//! replaces the running executable, spawns a detached `syscity restart
//! --pid <self>` helper, and cancels the gateway shutdown token so the daemon
//! exits and the helper starts the new binary.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::gateway::state::UpdatePhase;
use crate::gateway::GatewayState;

/// The background update run: check → download → verify → apply → restart.
pub(crate) async fn run_update_task(
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
    // `nocloud` preserves a `start --nocloud` CLI override across the restart:
    // the flag lives only on the command line, never in config.toml.
    #[cfg(feature = "cloud")]
    let nocloud = !state.config.read().await.cloud.enabled;
    #[cfg(not(feature = "cloud"))]
    let nocloud = false;
    if let Err(e) =
        crate::daemon::DaemonManager::spawn_restart_helper(&host, port, std::process::id(), nocloud)
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
pub(crate) async fn set_progress(
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
    use crate::gateway::state::UpdateProgress;

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
}
