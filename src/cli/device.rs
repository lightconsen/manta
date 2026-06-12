//! Device pairing management commands for Syscity
//!
//! Provides CLI access to device pairing state: list, approve, reject, revoke, qr.
//! Calls the daemon REST API at /api/v1/device/pairing/*.

use crate::error::{Result, SyscityError};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum DeviceCommands {
    /// List all pending and paired devices
    List,
    /// Approve a pending device pairing request
    Approve {
        /// Pairing code (e.g., A3F7K2X9)
        code: String,
    },
    /// Reject a pending device pairing request
    Reject {
        /// Pairing code
        code: String,
    },
    /// Revoke a paired device
    Revoke {
        /// Device ID
        id: String,
    },
    /// Show QR code SVG for a pairing request
    Qr {
        /// Pairing code
        code: String,
        /// Output file path (default: print SVG path)
        #[arg(short, long)]
        output: Option<String>,
    },
}

/// Run device commands
pub async fn run_device_command(command: &DeviceCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        DeviceCommands::List => {
            // List authorized devices
            let url = format!("{}/api/v1/device/pairing/authorized", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if let Some(devices) = body.get("devices").and_then(|d| d.as_array()) {
                        if devices.is_empty() {
                            println!("No paired devices.");
                        } else {
                            println!("Paired Devices:");
                            println!("{:<20} {:<20}", "Device ID", "Name");
                            println!("{}", "-".repeat(45));
                            for dev in devices {
                                let id = dev.get("device_id").and_then(|v| v.as_str()).unwrap_or("-");
                                let name = dev
                                    .get("display_name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("-");
                                println!("{:<20} {:<20}", id, name);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }

            // Also show pending requests
            let url = format!("{}/api/v1/device/pairing/pending", DAEMON_URL);
            if let Ok(resp) = client.get(&url).send().await {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                if let Some(pending) = body.get("pending").and_then(|r| r.as_array()) {
                    if !pending.is_empty() {
                        println!("\nPending Pairing Requests:");
                        println!("{:<12} {:<20}", "Code", "Device ID");
                        println!("{}", "-".repeat(35));
                        for req in pending {
                            let code = req.get("code").and_then(|c| c.as_str()).unwrap_or("-");
                            let dev_id = req.get("device_id").and_then(|c| c.as_str()).unwrap_or("-");
                            println!("{:<12} {:<20}", code, dev_id);
                        }
                    }
                }
            }
            Ok(())
        }
        DeviceCommands::Approve { code } => {
            let url = format!("{}/api/v1/device/pairing/approve", DAEMON_URL);
            let body = serde_json::json!({
                "code": code,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("Approved pairing request {}", code);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to approve: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        DeviceCommands::Reject { code } => {
            let url = format!("{}/api/v1/device/pairing/reject", DAEMON_URL);
            let body = serde_json::json!({ "code": code });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("Rejected pairing request {}", code);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to reject: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        DeviceCommands::Qr { code, output } => {
            let url = format!("{}/api/v1/device/pairing/qr/{}", DAEMON_URL, code);
            match client.get(&url).send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to get QR code: {}", text);
                        return Err(SyscityError::Internal("QR request failed".to_string()));
                    }
                    let svg = resp.text().await.unwrap_or_default();
                    if let Some(path) = output {
                        tokio::fs::write(&path, &svg).await
                            .map_err(|e| SyscityError::Internal(format!("Failed to write file: {}", e)))?;
                        println!("QR code saved to {}", path);
                    } else {
                        let tmp = std::env::temp_dir().join(format!("syscity-qr-{}.svg", code));
                        tokio::fs::write(&tmp, &svg).await
                            .map_err(|e| SyscityError::Internal(format!("Failed to write file: {}", e)))?;
                        println!("QR code saved to {}", tmp.display());
                        if cfg!(target_os = "macos") {
                            let _ = std::process::Command::new("open").arg(&tmp).spawn();
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        DeviceCommands::Revoke { id } => {
            let url = format!("{}/api/v1/device/pairing/revoke", DAEMON_URL);
            let body = serde_json::json!({
                "device_id": id,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("Revoked device {}", id);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to revoke: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
    }
}
