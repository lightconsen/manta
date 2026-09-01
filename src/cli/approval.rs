//! Tool approval management commands for Syscity
//!
//! Human-in-the-loop approval for high-risk tool executions (WS
//! `approvals.list` / `approvals.approve` / `approvals.deny`).

use clap::Subcommand;

use crate::cli::ws;
use crate::error::Result;

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
    match command {
        ApprovalCommands::List => {
            let body = ws::call("approvals.list", serde_json::json!({})).await?;
            let approvals = body.get("approvals").and_then(|a| a.as_array());
            match approvals {
                Some(approvals) if approvals.is_empty() => {
                    println!("No pending approvals.");
                }
                Some(approvals) => {
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
                None => println!("No pending approvals."),
            }
            Ok(())
        }
        ApprovalCommands::Approve { id } => {
            match ws::call("approvals.approve", serde_json::json!({ "id": id })).await {
                Ok(_) => {
                    println!("✅ Approved tool execution {}", id);
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Failed to approve: {}", e);
                    Err(e)
                }
            }
        }
        ApprovalCommands::Deny { id } => {
            match ws::call("approvals.deny", serde_json::json!({ "id": id })).await {
                Ok(_) => {
                    println!("❌ Denied tool execution {}", id);
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Failed to deny: {}", e);
                    Err(e)
                }
            }
        }
    }
}
