//! Tool approval management commands for Manta
//!
//! Human-in-the-loop approval for high-risk tool executions.

use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum ApprovalCommands {
    /// List pending tool approvals
    List,
    /// Approve a tool execution request
    Approve {
        /// Approval ID
        id: String,
    },
    /// Deny a tool execution request
    Deny {
        /// Approval ID
        id: String,
    },
}

/// Run approval commands
pub async fn run_approval_command(command: &ApprovalCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        ApprovalCommands::List => {
            let url = format!("{}/api/v1/approvals", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if let Some(approvals) = body.get("approvals").and_then(|a| a.as_array()) {
                        if approvals.is_empty() {
                            println!("No pending approvals.");
                        } else {
                            println!("Pending Tool Approvals:");
                            println!("{:<36} {:<20} {:<15} Message", "ID", "Tool", "Risk");
                            println!("{}", "-".repeat(100));
                            for app in approvals {
                                println!(
                                    "{:<36} {:<20} {:<15} {}",
                                    app.get("id").and_then(|c| c.as_str()).unwrap_or("-"),
                                    app.get("tool_name").and_then(|c| c.as_str()).unwrap_or("-"),
                                    app.get("risk_level")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("-"),
                                    app.get("message")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or("-")
                                        .chars()
                                        .take(40)
                                        .collect::<String>(),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        ApprovalCommands::Approve { id } => {
            let url = format!("{}/api/v1/approvals/{}/approve", DAEMON_URL, id);
            match client.post(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("✅ Approved tool execution {}", id);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to approve: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
        ApprovalCommands::Deny { id } => {
            let url = format!("{}/api/v1/approvals/{}/deny", DAEMON_URL, id);
            match client.post(&url).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        println!("❌ Denied tool execution {}", id);
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to deny: {}", text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
            Ok(())
        }
    }
}
