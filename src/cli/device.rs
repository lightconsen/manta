//! Device pairing management commands for Syscity
//!
//! Provides CLI access to device pairing state: list, approve, revoke.

use crate::error::{SyscityError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum DeviceCommands {
    /// List all paired devices
    List,
    /// Approve a pending device pairing request
    Approve {
        /// Pairing code (e.g., A3F7K)
        code: String,
    },
    /// Revoke a paired device
    Revoke {
        /// Device ID
        id: String,
    },
}

/// Run device commands
pub async fn run_device_command(command: &DeviceCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        DeviceCommands::List => {
            // List both pending and authorized
            let url = format!("{}/api/v1/pairing/authorized", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if let Some(devices) = body.get("devices").and_then(|d| d.as_array()) {
                        if devices.is_empty() {
                            println!("No paired devices.");
                        } else {
                            println!("Paired Devices:");
                            println!("{:<20} {:<15} {:<20}", "ID", "Channel", "Paired At");
                            println!("{}", "-".repeat(60));
                            for dev in devices {
                                println!(
                                    "{:<20} {:<15} {}",
                                    dev.get("id").and_then(|c| c.as_str()).unwrap_or("-"),
                                    dev.get("channel").and_then(|c| c.as_str()).unwrap_or("-"),
                                    dev.get("paired_at").and_then(|c| c.as_str()).unwrap_or("-"),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }

            // Also show pending
            let url = format!("{}/api/v1/pairing/pending", DAEMON_URL);
            if let Ok(resp) = client.get(&url).send().await {
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                if let Some(requests) = body.get("requests").and_then(|r| r.as_array()) {
                    if !requests.is_empty() {
                        println!("\nPending Pairing Requests:");
                        println!("{:<12} {:<15} {:<20}", "Code", "Channel", "User ID");
                        println!("{}", "-".repeat(50));
                        for req in requests {
                            println!(
                                "{:<12} {:<15} {}",
                                req.get("code").and_then(|c| c.as_str()).unwrap_or("-"),
                                req.get("channel").and_then(|c| c.as_str()).unwrap_or("-"),
                                req.get("user_id").and_then(|u| u.as_str()).unwrap_or("-"),
                            );
                        }
                    }
                }
            }
            Ok(())
        }
        DeviceCommands::Approve { code } => {
            let url = format!("{}/api/v1/pairing/approve", DAEMON_URL);
            let body = serde_json::json!({
                "code": code,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("✅ Approved pairing request {}", code);
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
        DeviceCommands::Revoke { id } => {
            let url = format!("{}/api/v1/pairing/revoke", DAEMON_URL);
            let body = serde_json::json!({
                "device_id": id,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("✅ Revoked device {}", id);
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
