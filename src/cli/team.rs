//! Team management commands for Manta

use crate::error::{MantaError, Result};
use clap::Subcommand;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum TeamCommands {
    /// List all teams
    List,
    /// Create a new team
    Create {
        /// Team name
        name: String,
        /// Team description
        #[arg(short, long)]
        description: Option<String>,
    },
    /// Show team details
    Show {
        /// Team name or ID
        name: String,
    },
    /// Delete a team
    Delete {
        /// Team name or ID
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Add a member to a team
    AddMember {
        /// Team name or ID
        team: String,
        /// Agent name to add
        agent: String,
        /// Member role
        #[arg(short, long, default_value = "member")]
        role: String,
    },
    /// Remove a member from a team
    RemoveMember {
        /// Team name or ID
        team: String,
        /// Agent name to remove
        agent: String,
    },
    /// List team members
    Members {
        /// Team name or ID
        team: String,
    },
    /// Assign a task to the team
    Assign {
        /// Team name or ID
        team: String,
        /// Task description
        task: String,
        /// Priority
        #[arg(short, long, default_value = "normal")]
        priority: String,
    },
}

/// Run team commands
pub async fn run_team_command(command: &TeamCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        TeamCommands::List => {
            let url = format!("{}/api/v1/teams", DAEMON_URL);
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
        TeamCommands::Create { name, description } => {
            let url = format!("{}/api/v1/teams", DAEMON_URL);
            let body = serde_json::json!({
                "name": name,
                "description": description,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Team '{}' created", name);
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to create team ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        TeamCommands::Show { name } => {
            let url = format!("{}/api/v1/teams/{}", DAEMON_URL, name);
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
        TeamCommands::Delete { name, force } => {
            if !force {
                println!("Delete team '{}'? Use --force to confirm.", name);
                return Ok(());
            }
            let url = format!("{}/api/v1/teams/{}", DAEMON_URL, name);
            match client.delete(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Team '{}' deleted", name);
                    } else {
                        eprintln!("Failed to delete team ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        TeamCommands::AddMember { team, agent, role } => {
            let url = format!("{}/api/v1/teams/{}/members", DAEMON_URL, team);
            let body = serde_json::json!({
                "agent": agent,
                "role": role,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Added '{}' to team '{}' as '{}'", agent, team, role);
                    } else {
                        eprintln!("Failed to add member ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        TeamCommands::RemoveMember { team, agent } => {
            let url = format!("{}/api/v1/teams/{}/members/{}", DAEMON_URL, team, agent);
            match client.delete(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Removed '{}' from team '{}'", agent, team);
                    } else {
                        eprintln!("Failed to remove member ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(MantaError::Internal(e.to_string()));
                }
            }
        }
        TeamCommands::Members { team } => {
            let url = format!("{}/api/v1/teams/{}/members", DAEMON_URL, team);
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
        TeamCommands::Assign { team, task, priority } => {
            let url = format!("{}/api/v1/teams/{}/tasks", DAEMON_URL, team);
            let body = serde_json::json!({
                "task": task,
                "priority": priority,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Task assigned to team '{}'", team);
                        println!("{}", text);
                    } else {
                        eprintln!("Failed to assign task ({}): {}", status, text);
                    }
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
