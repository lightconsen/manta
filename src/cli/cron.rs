//! Cron job management commands for Syscity

use clap::Subcommand;
use serde_json::json;

use crate::cli::ws;
use crate::error::Result;

#[derive(Debug, Subcommand)]
pub enum CronCommands {
    /// List all cron jobs
    List,
    /// Add a new cron job
    Add {
        /// Job name
        name: String,
        /// Cron schedule expression (e.g., "0 * * * *" for hourly)
        schedule: String,
        /// Command to execute
        command: String,
    },
    /// Remove a cron job
    Remove {
        /// Job name or ID
        name: String,
    },
    /// Enable a cron job
    Enable {
        /// Job name or ID
        name: String,
    },
    /// Disable a cron job
    Disable {
        /// Job name or ID
        name: String,
    },
    /// Run a cron job immediately
    Run {
        /// Job name or ID
        name: String,
    },
    /// Show cron job logs
    Logs {
        /// Job name or ID
        name: String,
        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: usize,
    },
}

/// Run cron commands (over WebSocket).
pub async fn run_cron_command(command: &CronCommands) -> Result<()> {
    match command {
        CronCommands::List => {
            let payload = ws::call("cron.list", json!({})).await?;
            println!("{}", payload);
        }
        CronCommands::Add { name, schedule, command } => {
            let body = json!({ "name": name, "schedule": schedule, "command": command });
            match ws::call("cron.add", body).await {
                Ok(payload) => {
                    println!("Cron job '{}' added", name);
                    println!("{}", payload);
                }
                Err(e) => {
                    eprintln!("Failed to add cron job: {}", e);
                    return Err(e);
                }
            }
        }
        CronCommands::Remove { name } => {
            match ws::call("cron.remove", json!({ "id": name })).await {
                Ok(_) => println!("Cron job '{}' removed", name),
                Err(e) => {
                    eprintln!("Failed to remove cron job: {}", e);
                    return Err(e);
                }
            }
        }
        CronCommands::Enable { name } => {
            match ws::call("cron.enable", json!({ "id": name })).await {
                Ok(_) => println!("Cron job '{}' enabled", name),
                Err(e) => {
                    eprintln!("Failed to enable cron job: {}", e);
                    return Err(e);
                }
            }
        }
        CronCommands::Disable { name } => {
            match ws::call("cron.disable", json!({ "id": name })).await {
                Ok(_) => println!("Cron job '{}' disabled", name),
                Err(e) => {
                    eprintln!("Failed to disable cron job: {}", e);
                    return Err(e);
                }
            }
        }
        CronCommands::Run { name } => match ws::call("cron.run", json!({ "id": name })).await {
            Ok(_) => println!("Cron job '{}' triggered", name),
            Err(e) => {
                eprintln!("Failed to run cron job: {}", e);
                return Err(e);
            }
        },
        CronCommands::Logs { name, lines: _ } => {
            let payload = ws::call("cron.logs", json!({ "id": name })).await?;
            println!("{}", payload);
        }
    }
    Ok(())
}
