//! Cron job management commands for Manta

use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

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

/// Run cron commands
pub async fn run_cron_command(command: &CronCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        CronCommands::List => {
            let url = format!("{}/api/v1/cron", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon at {}: {}", DAEMON_URL, e);
                    eprintln!("Is the daemon running? Try: manta start");
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        CronCommands::Add { name, schedule, command } => {
            let url = format!("{}/api/v1/cron", DAEMON_URL);
            let body = serde_json::json!({
                "name": name,
                "schedule": schedule,
                "command": command,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Cron job '{}' added", name);
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to add cron job ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        CronCommands::Remove { name } => {
            let url = format!("{}/api/v1/cron/{}", DAEMON_URL, name);
            match client.delete(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Cron job '{}' removed", name);
                    } else {
                        eprintln!("Failed to remove cron job ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        CronCommands::Enable { name } => {
            let url = format!("{}/api/v1/cron/{}/enable", DAEMON_URL, name);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Cron job '{}' enabled", name);
                    } else {
                        eprintln!("Failed to enable cron job ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        CronCommands::Disable { name } => {
            let url = format!("{}/api/v1/cron/{}/disable", DAEMON_URL, name);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Cron job '{}' disabled", name);
                    } else {
                        eprintln!("Failed to disable cron job ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        CronCommands::Run { name } => {
            let url = format!("{}/api/v1/cron/{}/run", DAEMON_URL, name);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Cron job '{}' triggered", name);
                    } else {
                        eprintln!("Failed to run cron job ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        CronCommands::Logs { name, lines: _ } => {
            let url = format!("{}/api/v1/cron/{}/logs", DAEMON_URL, name);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
    }
    Ok(())
}
