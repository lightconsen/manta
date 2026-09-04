//! Self-update commands for Syscity.
//!
//! Downloads the latest release from GitHub, verifies its SHA-256 checksum,
//! atomically replaces the running binary, and restarts the daemon.

use clap::Subcommand;

use crate::daemon::{DaemonConfig, DaemonManager};
use crate::error::{Result, SyscityError};
use crate::update::UpdateInfo;

#[derive(Debug, Subcommand)]
pub enum UpdateCommands {
    /// Check for a newer release (makes no changes)
    Check {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Download, verify, and install the latest release
    Do {
        /// Output machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Skip restarting the daemon after install
        #[arg(long)]
        no_restart: bool,
    },
}

/// Run update commands. Bare `syscity update` installs the latest release.
pub async fn run_update_command(command: &Option<UpdateCommands>) -> Result<()> {
    match command {
        Some(UpdateCommands::Check { json }) => check_online(*json).await,
        Some(UpdateCommands::Do { json, no_restart }) => run_install(*json, *no_restart).await,
        None => run_install(false, false).await,
    }
}

/// Check GitHub for a newer release and report the result.
async fn check_online(json: bool) -> Result<()> {
    let client = reqwest::Client::new();
    let info = crate::update::github::check_latest(&client, crate::VERSION).await?;
    print_info(&info, json);
    Ok(())
}

/// Check, download, verify, install, and optionally restart the daemon.
async fn run_install(json: bool, no_restart: bool) -> Result<()> {
    let client = reqwest::Client::new();
    let info = crate::update::github::check_latest(&client, crate::VERSION).await?;

    if !info.update_available {
        if json {
            println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
        } else {
            println!("Already up to date (v{}).", info.current);
        }
        return Ok(());
    }

    let target = crate::update::platform::asset_target().ok_or_else(|| {
        SyscityError::Unsupported(format!(
            "no release artifacts for {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;

    if !json {
        println!(
            "Downloading v{} for {}-{} ...",
            info.latest,
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }
    let checksum = crate::update::github::fetch_checksum(&client, &info.latest, target).await?;
    let url = crate::update::github::asset_download_url(&info.latest, target);
    let pkg = crate::update::download::download_and_verify(&client, &url, &checksum).await?;

    if !json {
        println!("Installing v{} ...", info.latest);
    }
    crate::update::apply::apply_binary(pkg.path())?;

    let restarted = if no_restart {
        false
    } else {
        restart_daemon().await?
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "current": info.current,
                "latest": info.latest,
                "update_available": true,
                "applied": true,
                "restarted": restarted,
            }))
            .unwrap_or_default()
        );
    } else {
        println!("✅ Updated to v{}.", info.latest);
        if restarted {
            println!("   Daemon restarted.");
        } else {
            println!("   Daemon not running; it will pick up the new version on next start.");
        }
    }
    Ok(())
}

/// Print an [`UpdateInfo`] as human text or JSON.
fn print_info(info: &UpdateInfo, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(info).unwrap_or_default());
    } else {
        println!("{info}");
    }
}

/// Restart the daemon so the freshly installed binary takes effect.
///
/// Returns whether a restart actually happened. If the daemon is not running
/// there is nothing to restart; the new binary is picked up on next start.
async fn restart_daemon() -> Result<bool> {
    let daemon_config = DaemonConfig {
        host: "127.0.0.1".to_string(),
        port: 18080,
        pid_file: crate::dirs::syscity_dir().join("syscity.pid"),
        remote_control_host: None,
        remote_control_user: None,
        remote_control_port: 0,
        remote_control_protocol: "ssh".to_string(),
        remote_control_key: None,
        headless: false,
        headless_display: String::new(),
        nocloud: false,
    };
    let daemon = DaemonManager::new(daemon_config)?;
    if daemon.is_running().await? {
        daemon.stop().await?;
        daemon.start().await?;
        Ok(true)
    } else {
        Ok(false)
    }
}
